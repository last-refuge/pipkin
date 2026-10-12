// Execute Pi's built-in scheduler/abort cleanup on synthetic SQLite profiles.
// This is an offline reference capture, never a native build/runtime dependency.
import { readFile, writeFile, mkdir, mkdtemp, copyFile, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import assert from 'node:assert/strict';

const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-orphan-import-reference.mjs PI_CHECKOUT OUTPUT_DIRECTORY');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
assert.equal(execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim(), pin);
const files = [
  'packages/durable/src/storage/sqlite/node.ts', 'packages/durable/src/harness/context.ts',
  'packages/durable/src/harness/scheduler.ts', 'packages/durable/src/harness/live.ts',
  'packages/durable/src/harness/generation.ts', 'packages/durable/src/harness/compaction.ts',
  'packages/durable/src/harness/tool.ts', 'packages/durable/src/harness/harness.ts',
  'packages/durable/src/storage/sqlite/storage.ts', 'packages/ai/src/providers/faux.ts',
];
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', 'packages/durable/src', 'packages/ai/src', 'packages/chord/src', 'packages/coding-agent/src/experimental/source-resolver.ts']);
const sources = {};
for (const file of files) sources[file] = createHash('sha256').update(await readFile(join(reference,file))).digest('hex');
const load = file => import(pathToFileURL(join(reference,file)));
await load('packages/coding-agent/src/experimental/source-resolver.ts');
const { openNodeSqliteStorage } = await load(files[0]);
const { captureContextBounds, deriveContext } = await load(files[1]);
const { Harness, createRegistry, GenerationTask, CompactionTask, ToolTask, LiveDoc, UserEntry, AssistantEntry } = await load('packages/durable/src/index.ts');
const { createModels, fauxProvider, fauxAssistantMessage } = await load('packages/ai/src/index.ts');
const hash = async path => createHash('sha256').update(await readFile(path)).digest('hex');
const context = {};
for (const kind of ['generation', 'compaction', 'tool', 'faulted']) {
  const temp = await mkdtemp(join(tmpdir(),'pipkin-orphan-reference-'));
  const dest = join(output,kind);
  await mkdir(dest,{recursive:true});
  try {
    const path = join(temp,'session.sqlite');
    const faux = fauxProvider();
    const models = createModels();
    models.setProvider(faux.provider);
    const reports = [];
    const faultModels = new Proxy(models, {get(target,property) {
      if (property === 'streamSimple') return () => ({
        async *[Symbol.asyncIterator]() {
          yield {type:'start',partial:fauxAssistantMessage('retained partial',{stopReason:'pending',timestamp:2})};
          await new Promise(resolve=>setTimeout(resolve,300));
        },
        result:async()=>({...fauxAssistantMessage('invalid final',{timestamp:3}),invalid:()=>{}}),
      });
      const value = Reflect.get(target,property,target);
      return typeof value === 'function' ? value.bind(target) : value;
    }});
    const options = {models:kind === 'faulted' ? faultModels : models,registry:createRegistry(),settings:{},now:()=>1700000000000,onReport:error=>reports.push(String(error))};
    let harness = await Harness.open(await openNodeSqliteStorage(path),options,context);
    let root = await harness.root(context,{agent:{model:{provider:'faux',modelId:'faux-1'}}});
    const rootId = root.id;
    let ids, blocked = null, settled, submission;
    if (kind === 'faulted') {
      harness.resume();
      const pending = await root.submit({type:'input',content:'Recover faulted work',requestId:'recover-faulted'},context);
      submission = await pending.wait(context);
      assert.equal(submission.reason,'faulted');
      assert.match(submission.detail,/non-JSON function/);
      const tasks = await harness.commit(tx=>tx.scanTasks({conversationId:rootId},10),context);
      settled = tasks.items[0];
      assert.equal(settled.state.outcome.status,'faulted');
      ids = {taskId:settled.id,submissionId:submission.id};
    } else {
    ids = await harness.commitWith(async tx => {
      const user = await tx.appendEntry(UserEntry,root.id,{model:[{role:'user',content:`Recover ${kind} work`,timestamp:1}]});
      const live = await tx.doc(LiveDoc,root.id);
      let submission, assistant;
      if (kind === 'generation') submission = await tx.createSubmission({conversationId:root.id,type:'input',status:'placed',entry:user.id,requestId:'recover-generation'});
      if (kind === 'tool') assistant = await tx.appendEntry(AssistantEntry,root.id,{model:[fauxAssistantMessage([{type:'toolCall',id:'interrupted-tool',name:'bash',arguments:{command:'echo must-not-run'}}],{stopReason:'toolUse',timestamp:2})]});
      const definition = {generation:GenerationTask,compaction:CompactionTask,tool:ToolTask}[kind].definition;
      const input = kind === 'compaction' ? {reason:'manual'} : kind === 'tool' ? {assistant:assistant.id,callId:'interrupted-tool'} : {};
      const taskId = await tx.createTask({definition:{...definition,version:definition.version+1}},input,{ownership:{kind:'conversation'},conversationId:root.id});
      if (kind === 'generation') {
        live.run = {taskId,inputs:[submission.id]};
        live.generation = {attempt:1,message:fauxAssistantMessage('retained partial',{stopReason:'pending',timestamp:2})};
      } else if (kind === 'compaction') live.compactions = [{taskId,reason:'manual',blocking:false,attempt:1}];
      else live.tools = [{taskId,callId:'interrupted-tool',name:'bash',status:'running',output:'unsettled output'}];
      return {taskId,...(submission ? {submissionId:submission.id} : {})};
    },context);
    await harness.close(context);
    await copyFile(path,join(dest,'blocked.sqlite'));
    harness = await Harness.open(await openNodeSqliteStorage(path),options,context);
    root = await harness.root(context);
    assert.equal(root.id,rootId);
    harness.resume();
    const inspection = await harness.inspect(context);
    blocked = inspection.tasks.find(task=>task.record.id===ids.taskId).state;
    assert.deepEqual(blocked,{kind:'blocked',reason:'task_too_old'});
    assert.equal(await harness.abortTask(ids.taskId,context),'marked');
    settled = await harness.waitForTask(ids.taskId,context);
    assert.deepEqual(settled.state.outcome,{status:'orphaned',reason:'task_too_old'});
    submission = ids.submissionId ? await (await harness.submission(ids.submissionId,context)).status(context) : null;
    if (submission) assert.equal(submission.reason,'task_too_old');
    }
    const live = await harness.snapshot(LiveDoc,root.id,context);
    await harness.close(context);
    const storage = await openNodeSqliteStorage(path);
    const entries = (await storage.scanEntries({conversationId:rootId},1000,undefined,context)).items.reverse();
    const bounds = await captureContextBounds(storage,rootId,context);
    const view = await deriveContext(storage,rootId,bounds,context);
    await storage.close(context);
    await copyFile(path,join(dest,'session.sqlite'));
    await writeFile(join(dest,'meta.json'),JSON.stringify({createdAt:1700000000000,cwd:'/synthetic/orphan-project'})+'\n');
    if (kind !== 'faulted') assert.deepEqual(reports,[]);
    await writeFile(join(dest,'reference.json'),JSON.stringify({reference:pin,sources,kind,rootId,...ids,blocked,settled,submission,live,entries,messages:view.messages,...(blocked ? {blockedSha256:await hash(join(dest,'blocked.sqlite'))} : {}),sqliteSha256:await hash(path)},null,2)+'\n');
    console.log(`${kind}: ${settled.state.outcome.status}; ${entries.length} entries, ${view.messages.length} context messages`);
  } finally { await rm(temp,{recursive:true,force:true}); }
}
