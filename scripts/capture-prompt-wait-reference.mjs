// Execute the pinned service's result projection with controlled durable submissions.
// This does not simulate or claim to execute the durable scheduler.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [source, output] = process.argv.slice(2);
assert(source && output, 'Usage: node scripts/capture-prompt-wait-reference.mjs PI_SOURCE OUTPUT_JSON');
const root = resolve(source);
const file = join(root, 'packages/coding-agent/src/experimental/services/agent-controller-provider.ts');
const sha = createHash('sha256').update(await readFile(file)).digest('hex');
assert.equal(sha, '773495ee99cb24511f536d5db61437679323299a5bdd01684815cf8c49c0362c');
await import(pathToFileURL(join(root, 'packages/coding-agent/src/experimental/source-resolver.ts')).href);
const { createAgentController } = await import(pathToFileURL(file).href);
const cases = [
  { name:'text_blocks_only', record:{type:'input',status:'done',answer:10}, entry:{model:[{role:'assistant',content:[{type:'text',text:'one'},{type:'thinking',thinking:'private'},{type:'text',text:'二'},{type:'toolCall',id:'ignored'}]}]} },
  { name:'missing_answer', record:{type:'input',status:'done'} },
  { name:'non_assistant', record:{type:'input',status:'done',answer:10}, entry:{model:[{role:'user',content:[]}]} },
  { name:'first_message_only', record:{type:'input',status:'done',answer:10}, entry:{model:[{role:'assistant',content:[{type:'text',text:'first'}]},{role:'assistant',content:[{type:'text',text:'second'}]}]} },
  ...['aborted','model_error','no_model','reset','stale'].map(reason=>({name:reason,record:{type:'input',status:'unanswered',reason}})),
  { name:'unknown', operationId:'1' },
  { name:'invalid_id', operationId:'bad' },
];
for (const c of cases) {
 const harness = { submission:async()=> c.record ? {wait:async()=>c.record} : undefined,
                   commit:async fn=>fn({entry:async()=>c.entry}) };
 const controller = createAgentController(harness, {id:1});
 try { c.result = await controller.waitForPrompt(c.operationId ?? '1', undefined); }
 catch(e) { c.error = e.message; }
}
await writeFile(resolve(output), JSON.stringify({referenceRevision:'d2a311097cbcf669e699479587332ae3988a49d0',sourceSha256:sha,nodeVersion:process.version,scope:'Original AgentController result projection with controlled submission/entry records; no live scheduler',cases},null,2)+'\n');
