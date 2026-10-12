// Execute the pinned catalog overlay and management retry functions with a local fetch stub.
// Rewrites import paths only; VERSION is a fixture constant because identity is not compared.
import { readFile, writeFile, mkdtemp, mkdir, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-catalog-reference.mjs PI_SOURCE OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== pin) throw new Error('Wrong reference pin');
const files = {
  'remote-catalog-provider.ts':'packages/coding-agent/src/core/remote-catalog-provider.ts',
  'management-http.ts':'packages/coding-agent/src/utils/management-http.ts',
  'pi-user-agent.ts':'packages/coding-agent/src/utils/pi-user-agent.ts',
  'model-operations.ts':'packages/ai/src/utils/model-operations.ts',
  'models-error.ts':'packages/ai/src/utils/models-error.ts',
  'diagnostics.ts':'packages/ai/src/utils/diagnostics.ts',
};
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',...Object.values(files)]);
const temp = await mkdtemp(join(tmpdir(),'pipkin-catalog-reference-'));
const savedFetch = globalThis.fetch, savedNow = Date.now;
try {
  const sources = {};
  for (const [local,path] of Object.entries(files)) {
    const source = await readFile(join(root,path),'utf8');
    sources[path] = createHash('sha256').update(source).digest('hex');
    await writeFile(join(temp,local),source.replace('"@earendil-works/pi-ai"','"./model-operations.ts"').replace('"../config.ts"','"./version.ts"').replace('"../utils/management-http.ts"','"./management-http.ts"').replace('"../utils/pi-user-agent.ts"','"./pi-user-agent.ts"'));
  }
  await writeFile(join(temp,'version.ts'),'export const VERSION = "fixture";\n');
  const {withRemoteCatalog} = await import(pathToFileURL(join(temp,'remote-catalog-provider.ts')));
  const generatedAt = Date.parse('2026-10-04T20:00:02.992Z'), now = Date.parse('2026-10-11T13:00:00Z');
  Date.now = ()=>now;
  const cached = {models:[{id:'cached',provider:'openai',api:'openai-completions'}],checkedAt:0,lastModified:now-1000,etag:'"cached"'};
  const cases = [
    {name:'newer-array',status:200,body:[{id:'new',api:'openai-completions'}],lastModified:'Sun, 11 Oct 2026 12:00:00 GMT'},
    {name:'typed-object',status:200,body:{models:[{id:'chat'},{id:'image',type:'image'},{id:'classifier',type:'classifier'},{id:'skip',type:'future'}]},lastModified:'Sun, 11 Oct 2026 12:00:00 GMT'},
    {name:'older',status:200,body:[{id:'old'}],lastModified:'Sun, 04 Oct 2026 12:00:00 GMT'},
    {name:'undated',status:200,body:[{id:'undated'}]},
    {name:'not-modified',status:304,stored:cached},
    {name:'missing',status:404,stored:cached},
    {name:'unsupported',status:501,stored:cached},
    {name:'transient',status:503,stored:cached},
    {name:'empty-validator',status:200,body:[{id:'new'}],lastModified:'Sun, 11 Oct 2026 12:00:00 GMT',stored:{...cached,models:[]}},
    {name:'fresh',status:200,body:[],stored:{...cached,checkedAt:now}},
    {name:'offline',status:200,body:[],stored:cached,offline:true},
  ];
  for (const c of cases) {
    const requests = [];
    globalThis.fetch = async (url,init)=>{
      requests.push({path:new URL(url).pathname,validator:new Headers(init?.headers).get('if-none-match')});
      return new Response(c.status === 304 ? null : JSON.stringify(c.body??{}),{status:c.status,headers:{...(c.lastModified?{'last-modified':c.lastModified}:{}),etag:'"new"'}});
    };
    const provider = withRemoteCatalog({id:'openai',getModels:()=>[],getAllModels:()=>[]},'https://catalog.example',generatedAt);
    let persisted = null, error = null;
    try {
      await provider.refreshModels({stored:c.stored,allowNetwork:!c.offline,signal:new AbortController().signal,publish:async publication=>{if(publication.persist)persisted=publication.persist;publication.update?.();return true;}});
    } catch (e) { error = e.message; }
    c.expected = {models:provider.getAllModels().map(m=>({id:m.id,type:m.type??'chat',provider:m.provider})),requests,persisted: persisted===null?null:{checked: persisted.checkedAt===now,lastModified:persisted.lastModified??null,etag:persisted.etag??null},error};
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,sources,generatedAt,now,scope:'Original overlay/retry functions; fetch/persistence recorder, no credentials or real network',cases},null,2)+'\n');
} finally {globalThis.fetch=savedFetch;Date.now=savedNow;await rm(temp,{recursive:true,force:true});}
