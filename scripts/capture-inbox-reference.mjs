// Execute the pinned Pi inbox functions with an in-memory transaction recorder.
// Uses an existing reference checkout and its installed dependencies; installs nothing.
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-inbox-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected reference revision');
const files = ['packages/durable/src/harness/inbox.ts', 'packages/durable/src/documents.ts', 'packages/durable/src/entries.ts'];
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', ...files]);
const sources = {};
for (const file of files) sources[file] = createHash('sha256').update(await readFile(join(reference,file))).digest('hex');
await import(pathToFileURL(join(reference, 'packages/coding-agent/src/experimental/source-resolver.ts')));
const {applyBoundary, withdrawQueuedInputs} = await import(pathToFileURL(join(reference, files[0])));
const user = (id, mode, text) => ({id, mode, content:[{type:'text',text}]});
const write = (id, head, summary) => ({id,mode:'write',entry:{kind:'pi.compaction',head,summary,model:[]}});
const items = [user(1,'steer','steer-one'), user(2,'followUp','follow'), write(3,3,'first'), user(4,'steer','steer-two'), write(5,3,'equal'), write(6,2,'backward'), write(7,4,'forward')];
const cases = [];
for (const [name, at, mode] of [['post-tools-one','postTools','one-at-a-time'], ['final-one','final','one-at-a-time'], ['final-all','final','all'], ['stop','stop','one-at-a-time']]) {
  const inbox = {items:structuredClone(items)};
  const placed = [], settled = [];
  let id = 100;
  const tx = {
    doc:async()=>inbox,
    appendEntry:async(...args)=>{
      const entry = args.at(-1);
      placed.push(entry.kind === 'pi.compaction' ? {summary:entry.summary, head:entry.head} : {user:entry.model[0].content[0].text});
      return {id:++id};
    },
    placeSubmission:()=>{},
    settleSubmission:(id,outcome)=>settled.push({id,...outcome}),
  };
  const boundary = {conversationId:1,inbox,steeringMode:mode,followUpMode:mode,head:undefined};
  const result = at === 'stop' ? await withdrawQueuedInputs(tx,1) : await applyBoundary(tx,boundary,at,1000);
  cases.push({name,items,placed,settled,remaining:inbox.items.map(i=>i.id),result:result??null});
}
await mkdir(dirname(output),{recursive:true});
await writeFile(output, JSON.stringify({reference:pin,scope:'Executed inbox functions; transaction recorder, not a full reference engine run',sources,cases},null,2)+'\n');
