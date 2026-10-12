// Actual pinned durable Harness queue boundaries; no live model or user profile.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join,dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
const [root,output]=process.argv.slice(2);
if(!root||!output) throw new Error('Usage: node scripts/capture-queue-runtime-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const reference='d2a311097cbcf669e699479587332ae3988a49d0';
assert.equal(execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim(),reference);
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--','packages/durable/src','packages/ai/src','packages/chord/src','packages/coding-agent/src/experimental/source-resolver.ts']);
const sources={};
for(const file of ['packages/durable/src/harness/generation.ts','packages/durable/src/harness/submissions.ts','packages/durable/src/harness/inbox.ts','packages/durable/src/harness/scheduler.ts']) sources[file]=createHash('sha256').update(await readFile(join(root,file))).digest('hex');
const load=file=>import(pathToFileURL(join(root,file)));
await load('packages/coding-agent/src/experimental/source-resolver.ts');
const {Harness,createRegistry,InboxDoc}=await load('packages/durable/src/index.ts');
const {MemoryStorage}=await load('packages/durable/src/storage/memory.ts');
const {createModels,fauxProvider,fauxAssistantMessage}=await load('packages/ai/src/index.ts');
const queued=[{key:'s1',mode:'steer'},{key:'f1',mode:'followUp'},{key:'s2',mode:'steer'},{key:'f2',mode:'followUp'}];
const cases=[
  {name:'final-one',first:'answer',queued},
  {name:'final-all',first:'answer',queued,steeringMode:'all',followUpMode:'all'},
  {name:'tools-one',first:'tools',queued},
  {name:'tools-all-steers',first:'tools',queued,steeringMode:'all'},
  {name:'tools-all-follow-ups',first:'tools',queued,followUpMode:'all'},
  {name:'withdraw-before-final',first:'answer',queued,withdraw:'f1'},
  {name:'withdraw-before-tools',first:'tools',queued,withdraw:'s1'},
  {name:'failure-follow-ups-then-prompt',first:'error',queued:queued.filter(q=>q.mode==='followUp'),wake:{key:'wake',mode:'reject'}},
  {name:'failure-steers-then-prompt',first:'error',queued:queued.filter(q=>q.mode==='steer'),wake:{key:'wake',mode:'reject'}},
  {name:'failure-mixed-then-steer',first:'error',queued,wake:{key:'wake',mode:'steer'}},
  {name:'failure-all-then-prompt',first:'error',queued,steeringMode:'all',followUpMode:'all',wake:{key:'wake',mode:'reject'}},
];
const text=m=>typeof m.content==='string'?m.content:m.content.filter(b=>b.type==='text').map(b=>b.text).join('');
const project=m=>[m.role,text(m),m.content instanceof Array?m.content.filter(b=>b.type==='toolCall').map(b=>b.name):[]];
for(const c of cases) {
  const models=createModels();models.setProvider(fauxProvider().provider);
  let release,reached;const gate=new Promise(resolve=>release=resolve),started=new Promise(resolve=>reached=resolve);
  const requests=[];let n=0;
  const controlled=new Proxy(models,{get(target,key) {
    if(key==='streamSimple') return (_model,ctx)=>{
      requests.push(ctx.messages.filter(m=>m.role!=='system').map(project));
      const first=n++===0;
      const response=async()=>{
        if(first) {reached();await gate;}
        if(first&&c.first==='error') return fauxAssistantMessage('',{stopReason:'error',errorMessage:'invalid request',timestamp:1});
        if(first&&c.first==='tools') return fauxAssistantMessage([{type:'toolCall',id:'call-one',name:'not-installed',arguments:{}}],{stopReason:'toolUse',timestamp:1});
        return fauxAssistantMessage('answer:'+text(ctx.messages.filter(m=>m.role==='user').at(-1)),{timestamp:1});
      };
      return {async *[Symbol.asyncIterator](){},result:response};
    };
    const value=Reflect.get(target,key,target);return typeof value==='function'?value.bind(target):value;
  }});
  const settings={retry:{enabled:false},compaction:{enabled:false},steeringMode:c.steeringMode??'one-at-a-time',followUpMode:c.followUpMode??'one-at-a-time'};
  const reports=[];
  const harness=await Harness.open(new MemoryStorage(),{models:controlled,registry:createRegistry(),settings,onReport:e=>reports.push(String(e))},{});
  try {
    const root=await harness.root({},{agent:{model:{provider:'faux',modelId:'faux-1'}}});harness.resume();
    const inputs=new Map();
    inputs.set('main',await root.submit({type:'input',content:'main',requestId:'main'},{}));await started;
    await assert.rejects(root.submit({type:'input',content:'busy-reject',requestId:'busy-reject',whenBusy:'reject'},{}),/busy/);
    for(const q of c.queued) inputs.set(q.key,await root.submit({type:'input',content:q.key,requestId:q.key,whenBusy:q.mode},{}));
    if(c.withdraw) assert.equal(await inputs.get(c.withdraw).abort({}),'aborted');
    const queue=async()=>{
      const inbox=await harness.snapshot(InboxDoc,root.id,{});
      return inbox.items.map(i=>({key:[...inputs].find(([,s])=>s.id===i.id)[0],mode:i.mode}));
    };
    c.before=await queue();release();
    await inputs.get('main').wait({});await harness.waitForIdle({});
    c.afterFirst={queue:await queue(),requests:structuredClone(requests)};
    if(c.wake) {
      inputs.set(c.wake.key,await root.submit({type:'input',content:c.wake.key,requestId:c.wake.key,whenBusy:c.wake.mode},{}));
      await inputs.get(c.wake.key).wait({});await harness.waitForIdle({});
    }
    const entries=(await root.entries({},1000,undefined,{})).items.reverse();
    const outcomes={};
    for(const [key,input] of inputs) {
      const s=await input.status({});
      outcomes[key]={status:s.status,reason:s.reason??null,answer:s.answer?text(entries.find(e=>e.id===s.answer).model[0]):null};
    }
    c.expected={requests,queue:await queue(),history:entries.flatMap(e=>e.model??[]).filter(m=>m.role!=='system').map(project),outcomes};
    assert.deepEqual(reports,[]);console.log(`${c.name}: ${requests.length} requests; ${c.afterFirst.queue.length} held after first phase`);
  } finally {await harness.close({});}
}
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference,sources,node:process.version,scope:'Executed durable Harness with a gated first model response, real inbox/submission boundaries and deterministic normalized replies. No external services.',cases},null,2)+'\n');
