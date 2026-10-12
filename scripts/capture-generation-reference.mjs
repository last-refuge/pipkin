// Offline execution of the pinned durable Harness with deterministic normalized responses.
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join,dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
const [root,output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-generation-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
assert.equal(execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim(),reference);
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--','packages/durable/src','packages/ai/src','packages/chord/src','packages/coding-agent/src/experimental/source-resolver.ts','packages/coding-agent/src/experimental/durable/harness-setup.ts']);
const sources = {};
for (const file of ['packages/durable/src/harness/generation.ts','packages/durable/src/harness/tool.ts','packages/durable/src/harness/compaction.ts','packages/durable/src/harness/context.ts','packages/ai/src/utils/retry.ts','packages/ai/src/utils/overflow.ts','packages/coding-agent/src/experimental/durable/harness-setup.ts']) {
  sources[file] = createHash('sha256').update(await readFile(join(root,file))).digest('hex');
}
const load = file=>import(pathToFileURL(join(root,file)));
await load('packages/coding-agent/src/experimental/source-resolver.ts');
const {Harness,createRegistry} = await load('packages/durable/src/index.ts');
const {MemoryStorage} = await load('packages/durable/src/storage/memory.ts');
const {CodingTools} = await load('packages/durable/src/tools/index.ts');
const {NodeExecutionEnv} = await load('packages/durable/src/env/node.ts');
const {createModels,fauxProvider,fauxAssistantMessage} = await load('packages/ai/src/index.ts');
const {isRetryableAssistantError,retryDelayMs} = await load('packages/ai/src/utils/retry.ts');
const {isContextOverflow} = await load('packages/ai/src/utils/overflow.ts');
const text = (value,stopReason='stop')=>fauxAssistantMessage(value,{stopReason,timestamp:1});
const error = value=>({...text('', 'error'),errorMessage:value});
const call = (name,stopReason='toolUse')=>text([{type:'toolCall',id:'call-one',name,arguments:{path:'must-not-write.txt',content:'unexpected'}}],stopReason);
const retry = {enabled:true,maxRetries:2,baseDelayMs:1,maxAgentDelayMs:2};
const cases = [
  {name:'plain-answer',responses:[text('answer')]},
  {name:'unknown-tool',responses:[call('not-installed'),text('handled unavailable tool')]},
  {name:'unoffered-subagent',responses:[call('subagent'),text('handled unavailable subagent')]},
  {name:'stop-with-tool-call',responses:[call('write','stop')]},
  {name:'length-with-tool-call',responses:[call('write','length')]},
  {name:'empty-tool-use',responses:[text('empty round','toolUse')]},
  {name:'retry-then-answer',responses:[error('503 overloaded'),error('connection reset before headers'),text('recovered')]},
  {name:'retry-exhausted',responses:Array.from({length:4},()=>error('503 overloaded'))},
  {name:'retry-disabled',retry:{...retry,enabled:false},responses:[error('503 overloaded'),text('unexpected')]},
  {name:'billing-beats-retry',responses:[error('429 billing limit'),text('unexpected')]},
  {name:'subscription-limit',responses:[error('429 subscription_sharing_usage_limit_exceeded'),text('unexpected')]},
  {name:'subscription-unavailable',responses:[error('subscription_sharing_usage_unavailable'),text('recovered')]},
  {name:'overflow-beats-retry-disabled-compaction',responses:[error('503 prompt too long'),text('unexpected')]},
  {name:'overflow-no-summary-prefix',compaction:true,responses:[error('503 prompt too long'),text('unexpected')]},
  {name:'rate-limit-excludes-overflow',responses:[error('429 rate limit: prompt too long'),text('recovered')]},
  {name:'cerebras-bodyless-overflow',responses:[{...error('413 status code (no body) 503 overloaded'),provider:'cerebras'},text('unexpected')]},
  {name:'other-bodyless-retry',responses:[error('413 status code (no body) 503 overloaded'),text('recovered')]},
  {name:'overflow-compact-recover',seed:4,compaction:true,responses:[error('503 prompt too long'),text('recovered')],summaries:[text('retained summary')]},
  {name:'overflow-after-compaction',seed:4,compaction:true,responses:[error('503 prompt too long'),error('503 prompt too long'),text('unexpected')],summaries:[text('retained summary')]},
  {name:'overflow-failed-summary',seed:4,compaction:true,responses:[error('503 prompt too long'),text('unexpected')],summaries:[error('401 invalid summary credential')]},
  {name:'silent-overflow-stays-answer',responses:[{...text('retained answer'),usage:{...text('').usage,input:300000}}]},
  {name:'length-zero-output-stays-answer',responses:[{...text('','length'),usage:{...text('').usage,input:200000,output:0}}]},
  {name:'provider-aborted-without-user-stop',responses:[{...text('provider partial','aborted'),errorMessage:'provider aborted'},text('unexpected')]},
  {name:'provider-aborted-without-error-text',responses:[text('provider partial','aborted'),text('unexpected')]},
  {name:'provider-error-without-error-text',responses:[text('','error'),text('unexpected')]},
];
// Timestamp, provider/model identity and accounting belong to separate contracts. Keep
// control reason, error text, content/arguments and complete tool diagnostic blocks here.
function project(m) {
  const out = {role:m.role,content:typeof m.content==='string' ? [{type:'text',text:m.content}] : m.content};
  for (const key of ['stopReason','errorMessage','toolCallId','toolName','isError']) if (m[key] !== undefined) out[key]=m[key];
  return out;
}
for (const c of cases) {
  const cwd = await mkdtemp(join(tmpdir(),'pipkin-generation-reference-'));
  const env = new NodeExecutionEnv({cwd});
  const models = createModels();
  models.setProvider(fauxProvider().provider);
  let seed = true, index = 0, summaryIndex = 0;
  const requests = [], reports = [];
  const respond = (ctx,summary) => {
    if (seed) return text('seed answer');
    const response = structuredClone((summary ? c.summaries?.[summaryIndex++] : c.responses[index++]) ?? error('Unexpected extra provider request'));
    requests.push({summary,messages:ctx.messages.filter(m=>m.role!=='system').map(project),response:project(response)});
    return response;
  };
  const controlled = new Proxy(models,{get(target,key) {
    if (key === 'streamSimple') return (_model,ctx)=>{
      const message = respond(ctx,false);
      return {async *[Symbol.asyncIterator]() {},result:async()=>message};
    };
    if (key === 'completeSimple') return async (_model,ctx)=>respond(ctx,true);
    const value=Reflect.get(target,key,target);
    return typeof value==='function' ? value.bind(target) : value;
  }});
  const registry=createRegistry();registry.install(CodingTools);
  const settings={retry:c.retry??retry,compaction:{enabled:false,keepRecentTokens:5,reserveTokens:1000,backgroundTokens:0}};
  const harness=await Harness.open(new MemoryStorage(),{models:controlled,registry,settings,env:()=>env,onReport:error=>reports.push(String(error))},{});
  try {
    const conversation=await harness.root({},{agent:{model:{provider:'faux',modelId:'faux-1'}}});
    harness.resume();
    for(let i=0;i<(c.seed??0);i++) await (await conversation.submit({type:'input',content:'earlier conversation with several words'},{})).wait({});
    seed=false;settings.compaction.enabled=!!c.compaction;
    const submission=await (await conversation.submit({type:'input',content:'perform the requested turn',requestId:'main'},{})).wait({});
    const entries=(await conversation.entries({},1000,undefined,{})).items.reverse();
    const tasks=await harness.commit(tx=>tx.scanTasks({conversationId:conversation.id},100),{});
    c.expected={status:submission.status,reason:submission.reason??null,detail:submission.detail??null,requests,history:entries.flatMap(e=>e.model??[]).filter(m=>m.role!=='system').map(project),compactions:entries.filter(e=>e.kind==='pi.compaction').length,tasks:tasks.items};
    assert.deepEqual(reports,[]);
    await assert.rejects(readFile(join(cwd,'must-not-write.txt')),{code:'ENOENT'});
    console.log(`${c.name}: ${submission.status}; ${index} generation, ${summaryIndex} summary requests`);
  } finally {await harness.close({});await env.cleanup({});await rm(cwd,{recursive:true,force:true});}
}
const classifications=[];
for (const c of cases) for (const message of c.responses) classifications.push({case:c.name,message,retry:isRetryableAssistantError(message),overflow:isContextOverflow(message,200000)});
const delays=[0,1,2,3,10,53,1024].map(attempt=>({attempt,value:retryDelayMs({baseDelayMs:7,maxAgentDelayMs:1000},attempt)}));
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference,sources,node:process.version,scope:'Actual durable Harness with CodingTools and deterministic normalized provider responses; no cloud requests. Raw retry/overflow classifiers also executed.',retry,cases,classifications,delays},null,2)+'\n');
