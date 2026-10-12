// Execute the pinned SettingsManager in-memory, with unused I/O dependencies isolated.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-compaction-settings-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const path = 'packages/coding-agent/src/core/settings-manager.ts';
const textPath = 'packages/coding-agent/src/utils/text.ts';
const retryPath = 'packages/ai/src/utils/retry.ts';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',path,textPath,retryPath]);
const source = await readFile(join(root,path),'utf8');
const textSource = await readFile(join(root,textPath),'utf8');
const retrySource = await readFile(join(root,retryPath),'utf8');
const temp = await mkdtemp(join(tmpdir(),'pipkin-compaction-settings-'));
try {
  const rewritten = source
    .replace('from "@earendil-works/pi-ai";', 'from "./stubs.mjs";')
    .replace('import lockfile from "proper-lockfile";', 'import {lockfile} from "./stubs.mjs";')
    .replace('from "../config.ts";', 'from "./stubs.mjs";')
    .replace('from "../utils/paths.ts";', 'from "./stubs.mjs";')
    .replace('from "../utils/text.ts";', 'from "./text.ts";')
    .replace('from "./http-dispatcher.ts";', 'from "./stubs.mjs";');
  await writeFile(join(temp,'settings.ts'),rewritten);
  await writeFile(join(temp,'text.ts'),textSource);
  await writeFile(join(temp,'retry.ts'),retrySource);
  await writeFile(join(temp,'stubs.mjs'),`
    const unused = () => {throw new Error('Unexercised dependency called');};
    export {DEFAULT_MAX_AGENT_RETRY_DELAY_MS} from './retry.ts';
    export const DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300000, CONFIG_DIR_NAME = '.pi';
    export const getAgentDir = unused, normalizePath = unused, resolvePath = unused, parseHttpIdleTimeoutMs = unused;
    export const lockfile = {lockSync: unused};
  `);
  const {SettingsManager, InMemorySettingsStorage} = await import(pathToFileURL(join(temp,'settings.ts')));
  const cases = [
    {name:'defaults',compaction:{}},
    {name:'project-clears-retry',compaction:{},extraGlobal:{retry:{maxRetries:8,baseDelayMs:7}},project:{retry:null}},
    {name:'project-clears-retry-field',compaction:{},extraGlobal:{retry:{enabled:false,maxRetries:8,baseDelayMs:7}},project:{retry:{maxRetries:null}}},
    {name:'global-null-retry-fields',compaction:{},extraGlobal:{retry:{enabled:null,maxRetries:null,baseDelayMs:null,maxAgentDelayMs:null}}},
    {name:'null-policy',compaction:null},
    {name:'array-policy',compaction:[]},
    {name:'string-policy',compaction:'unused'},
    {name:'ordinary-keep-invalid',compaction:{keepRecentTokens:-1}},
    {name:'both-ordinary-invalid',compaction:{reserveTokens:-1,keepRecentTokens:-2}},
    {name:'project-null-entry',compaction:{modelOverrides:{'p/x':{keepRecentTokens:5}}},project:{compaction:{modelOverrides:{'p/x':null}}}},
    {name:'ordinary',compaction:{reserveTokens:100,keepRecentTokens:5,enabled:false}},
    {name:'integral-float',compaction:{reserveTokens:1.0,keepRecentTokens:0}},
    {name:'safe-boundary',compaction:{reserveTokens:9007199254740991}},
    {name:'partial-override',compaction:{reserveTokens:100,modelOverrides:{'p/x':{keepRecentTokens:3}}}},
    {name:'case-sensitive',key:'P/X',compaction:{modelOverrides:{'p/x':null}}},
    {name:'unrelated-invalid',compaction:{modelOverrides:{'other/model':{reserveTokens:-1}}}},
    {name:'ordinary-validated-before-override',compaction:{reserveTokens:-1,modelOverrides:{'p/x':{reserveTokens:100}}}},
    {name:'project-repairs-global',compaction:{reserveTokens:-1,modelOverrides:{'p/x':{keepRecentTokens:5}}},project:{compaction:{reserveTokens:100}}},
    {name:'project-invalid-override',compaction:{reserveTokens:100},project:{compaction:{modelOverrides:{'p/x':{keepRecentTokens:-2}}}}},
    {name:'enabled-null',compaction:{enabled:null}},
    {name:'enabled-zero',compaction:{enabled:0}},
    {name:'enabled-string',compaction:{enabled:'false'}},
  ];
  for (const [name,value] of [['negative',-1],['fraction',1.5],['numeric-string','10'],['null',null],['boolean',false],['object',{}],['array',[null,3]],['unsafe',9007199254740992],['scientific-large',1e21],['scientific-small',1e-7]]) {
    cases.push({name:'ordinary-'+name,compaction:{reserveTokens:value}});
    cases.push({name:'override-'+name,compaction:{modelOverrides:{'p/x':{keepRecentTokens:value}}}});
  }
  for (const entry of [null,[],3,'bad']) cases.push({name:'invalid-entry-'+String(entry),compaction:{modelOverrides:{'p/x':entry}}});
  for (const c of cases) {
    c.key ??= 'p/x';
    c.global = {defaultProvider:'kept-provider',defaultModel:'kept-model',compaction:c.compaction,...c.extraGlobal};delete c.compaction;delete c.extraGlobal;
    c.project ??= {};
    const storage = new InMemorySettingsStorage();
    storage.withLock('global',()=>JSON.stringify(c.global));storage.withLock('project',()=>JSON.stringify(c.project));
    const settings = SettingsManager.fromStorage(storage);
    const [provider,...id] = c.key.split('/');
    try { c.effective = settings.getCompactionSettings({provider,id:id.join('/')}); c.effective.enabled = Boolean(c.effective.enabled); }
    catch (e) { c.error = e.message; }
    c.retry = settings.getRetrySettings();c.defaultProvider = settings.getDefaultProvider();c.defaultModel = settings.getDefaultModel();
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,node:process.version,sourceSha256:createHash('sha256').update(source).digest('hex'),textSha256:createHash('sha256').update(textSource).digest('hex'),retrySha256:createHash('sha256').update(retrySource).digest('hex'),scope:'Actual in-memory SettingsManager merge/getters; import paths rewritten, unused I/O/network dependencies throw if called; enabled normalized to consumer truthiness',cases},null,2)+'\n');
} finally {await rm(temp,{recursive:true,force:true});}
