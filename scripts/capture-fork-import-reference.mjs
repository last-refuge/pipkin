// Execute pinned Pi storage and context derivation; synthetic records, no live profile.
import { readFile, writeFile, mkdir, mkdtemp, copyFile, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-fork-import-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected reference revision');
const files = ['packages/durable/src/storage/sqlite/node.ts', 'packages/durable/src/harness/context.ts', 'packages/durable/src/session/forks.ts', 'packages/durable/src/ids.ts', 'packages/durable/src/errors.ts', 'packages/durable/src/storage/sqlite/storage.ts', 'packages/durable/src/storage/sqlite/migrations.ts'];
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', ...files, 'packages/chord/src/delta']);
const sources = {};
for (const file of files) sources[file] = createHash('sha256').update(await readFile(join(reference,file))).digest('hex');
await import(pathToFileURL(join(reference, 'packages/coding-agent/src/experimental/source-resolver.ts')));
const { openNodeSqliteStorage } = await import(pathToFileURL(join(reference, files[0])));
const { captureContextBounds, deriveContext } = await import(pathToFileURL(join(reference, files[1])));
const { prepareForkDocumentCopies } = await import(pathToFileURL(join(reference, files[2])));
const temp = await mkdtemp(join(tmpdir(),'pipkin-fork-reference-'));
try {
const storage = await openNodeSqliteStorage(join(temp,'session.sqlite')), conversations = [], entries = [];
const commit = async (type, value) => { await storage.commit([{type,value}], {}); (type === 'conversation' ? conversations : entries).push(value); };
const user = (id, conversationId, text) => ({id,conversationId,kind:'pi.user',model:[{role:'user',content:text,timestamp:id}]});
const assistant = (id, conversationId) => ({id,conversationId,kind:'pi.assistant',model:[{role:'assistant',content:[{type:'toolCall',id:'read-one',name:'read',arguments:{path:'a.rs'}}],api:'openai-completions',provider:'openai',model:'test',usage:{input:1,output:1,cacheRead:0,cacheWrite:0,totalTokens:2,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'toolUse',timestamp:id}]});
await commit('conversation', {id:1});
await storage.commit([{type:'document.create',record:{id:2,kind:'pi.agent',scope:{kind:'conversation',conversationId:1},history:'rewindable',fork:'asOf'},content:{kind:'base',version:1,value:{model:{provider:'openai',modelId:'before'},thinkingLevel:'low'}}}], {});
await commit('entry',user(3,1,'shared start'));
await commit('entry',assistant(4,1));
await storage.commit([{type:'document.change',id:2,content:{kind:'base',version:1,value:{model:{provider:'openai',modelId:'after'},thinkingLevel:'high'}}}], {});
// Copy asOf settings through the actual fork document selector and storage copy path.
const copies = await prepareForkDocumentCopies(storage,1,4,10,{});
const first = {id:10,parent:{conversationId:1,at:4}};
await storage.commit([{type:'conversation',value:first},...copies.map(copy=>({type:'document.copy',...copy}))],{});
conversations.push(first);
await commit('entry',{id:11,conversationId:1,kind:'pi.tool-result',model:[{role:'toolResult',toolCallId:'read-one',toolName:'read',content:'parent-only result',isError:false,timestamp:11}]});
await commit('entry',user(12,1,'parent after fork'));
await commit('entry',user(13,10,'branch after missing tool result'));
await commit('conversation',{id:20,parent:{conversationId:10,at:3}});
await commit('entry',user(21,20,'nested rewind to ancestor'));
await commit('conversation',{id:30,parent:{conversationId:10,at:13}});
await commit('entry',{id:31,conversationId:30,kind:'pi.cancelled-input',edits:[{target:3,action:'omit'}]});
await commit('entry',user(32,30,'nested omission'));
await commit('conversation',{id:40,parent:{conversationId:30,at:32}});
await commit('entry',{id:41,conversationId:40,kind:'pi.compaction',head:32,model:[{role:'user',content:'The conversation history before this point was compacted into the following summary:\n\n<summary>\nbranch summary\n</summary>',timestamp:41}]});
await commit('entry',user(42,40,'after compaction'));
await commit('conversation',{id:50,parent:{conversationId:40,at:42}});
await commit('entry',{id:51,conversationId:50,kind:'pi.reset',head:51,model:[{role:'user',content:'reset handoff',timestamp:51}]});
await commit('entry',user(52,50,'after reset'));
await commit('conversation',{id:60,parent:{conversationId:40,at:42}}); // No local entries.
const documents = [], expected = [];
for (const {id} of conversations) {
  const visible = (await storage.scanEntries({conversationId:id},1000,undefined,{})).items.reverse();
  const bounds = await captureContextBounds(storage,id,{});
  const view = await deriveContext(storage,id,bounds,{});
  expected.push({id,visibleIds:visible.map(e=>e.id),messages:view.messages});
  const docs = await storage.scanDocuments({scope:{kind:'conversation',conversationId:id},at:'current'},1000,undefined,{});
  for (const record of docs.items) {
    const stored = await storage.document(record.id,'current',{});
    documents.push({record,version:stored.version,value:stored.value});
  }
}
await mkdir(dirname(output),{recursive:true});
await storage.close({});
await copyFile(join(temp,'session.sqlite'),join(dirname(output),'session.sqlite'));
await writeFile(join(dirname(output),'meta.json'),JSON.stringify({createdAt:1700000000000,cwd:'/synthetic/fork-project'})+'\n');
const sqliteSha256 = createHash('sha256').update(await readFile(join(temp,'session.sqlite'))).digest('hex');
await writeFile(output, JSON.stringify({reference:pin,scope:'Executed Pi SQLite storage, fork document copying, and context derivation; synthetic settled histories',sources,sqliteSha256,conversations,entries,documents,expected},null,2)+'\n');
} finally { await rm(temp,{recursive:true,force:true}); }
