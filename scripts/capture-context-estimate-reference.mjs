// Execute the pinned estimator and tool-result ordering over normalized context views.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join,dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import assert from 'node:assert/strict';
const [root,output]=process.argv.slice(2);
if(!root||!output) throw new Error('Usage: node scripts/capture-context-estimate-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const reference='d2a311097cbcf669e699479587332ae3988a49d0';
assert.equal(execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim(),reference);
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--','packages/durable/src','packages/ai/src','packages/coding-agent/src/experimental/source-resolver.ts']);
const sources={};
for(const file of ['packages/durable/src/harness/compaction.ts','packages/durable/src/harness/context.ts','packages/ai/src/utils/estimate.ts']) sources[file]=createHash('sha256').update(await readFile(join(root,file))).digest('hex');
const load=file=>import(pathToFileURL(join(root,file)));
await load('packages/coding-agent/src/experimental/source-resolver.ts');
const {estimateContext}=await load('packages/durable/src/harness/compaction.ts');
const {orderToolResults}=await load('packages/durable/src/harness/context.ts');
const usage=(totalTokens=0)=>({input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}});
const user=text=>({role:'user',content:[{type:'text',text}],timestamp:0});
const assistant=(text,calls=[],tokens=0)=>({role:'assistant',content:[{type:'text',text},...calls.map(id=>({type:'toolCall',id,name:'read',arguments:{}}))],api:'faux',provider:'faux',model:'faux',usage:usage(tokens),stopReason:calls.length?'toolUse':'stop',timestamp:0});
const result=(id,text)=>({role:'toolResult',toolCallId:id,toolName:'read',content:[{type:'text',text}],isError:false,timestamp:0});
const entry=(id,message)=>({id,atMs:0,kind:{kind:'message',message}});
const entries=messages=>messages.map((message,index)=>entry(index+1,message));
const cases=[];
const add=(name,messages,extra=[])=>cases.push({name,entries:entries(messages),extra});
add('empty',[]);
add('unicode-rounding',[user('🎉🎉🎉'),assistant('abcde')],[user('😀abc')]);
add('latest-usage',[user('earlier'),assistant('first',[],100),user('later'),assistant('second',[],200),user('tail')]);
const components=assistant('components');components.usage={...usage(),input:10,output:2,cacheRead:3,cacheWrite:4};
add('usage-components',[components,user('abcde')]);
const repeated=[user('old'),assistant('calls',['same','same','missing'],123),result('same','x'.repeat(64)),result('same','ignored duplicate'),user('next')];
add('repeated-and-missing',repeated,[user('extra')]);
add('unmeasured-repeated',repeated.map(m=>m.role==='assistant'?{...m,usage:usage()}:m));
add('stray-and-late-result',[result('orphan','ignored'),assistant('call',['lost'],50),user('later'),assistant('next'),result('lost','too late')]);
add('same-id-later-round',[assistant('a',['same'],20),result('same','first'),assistant('b',['same'],30),result('same','second')]);
add('images-and-thinking',[{...user('hello'),content:[{type:'text',text:'hello'},{type:'image',mimeType:'image/png',data:'AA=='}]},{...assistant(''),content:[{type:'thinking',thinking:'thought 🦀',signature:'opaque'}]}]);
const summary='The conversation history before this point was compacted into the following summary:\n\n<summary>\nS\n</summary>';
for(const newer of [false,true]) {
 const c={name:newer?'usage-after-head':'usage-before-head',entries:[{id:10,atMs:0,kind:{kind:'compaction',summary:'S',first_kept:5,tokens_before:999}},entry(5,assistant('old',[],900)),entry(11,user('tail'))],extra:[user('extra')]};
 if(newer)c.entries.push(entry(12,assistant('new',[],100)));
 cases.push(c);
}
cases.push({name:'reset-handoff',entries:[{id:10,atMs:0,kind:{kind:'reset',handoff:user('handoff')}},entry(11,user('tail'))],extra:[]});
for(const c of cases) {
 const head=['compaction','reset'].includes(c.entries[0]?.kind.kind)?c.entries[0]:undefined;
 const contributions=c.entries.map(e=>e.kind.kind==='message'?[e.kind.message]:e.kind.kind==='compaction'?[user(summary)]:e.kind.handoff?[e.kind.handoff]:[]);
 const messages=orderToolResults(contributions.flat());
 c.expected=estimateContext({head,entries:c.entries,contributions,messages},c.extra);
 c.ordered=messages;
}
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference,sources,scope:'Pure estimator and tool-result ordering on normalized context views; not storage/context-edit derivation',cases},null,2)+'\n');
console.log(`${cases.length} reference context estimates captured`);
