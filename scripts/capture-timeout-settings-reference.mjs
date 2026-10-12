// Execute pinned timeout getters and parser with in-memory settings and no network I/O.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-timeout-settings-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const path = 'packages/coding-agent/src/core/settings-manager.ts';
const textPath = 'packages/coding-agent/src/utils/text.ts';
const retryPath = 'packages/ai/src/utils/retry.ts';
const httpPath = 'packages/coding-agent/src/core/http-dispatcher.ts';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',path,textPath,retryPath,httpPath]);
const source = await readFile(join(root,path),'utf8');
const textSource = await readFile(join(root,textPath),'utf8');
const retrySource = await readFile(join(root,retryPath),'utf8');
const httpSource = await readFile(join(root,httpPath),'utf8');
const temp = await mkdtemp(join(tmpdir(),'pipkin-timeout-settings-'));
try {
  const rewritten = source
    .replace('from "@earendil-works/pi-ai";', 'from "./stubs.mjs";')
    .replace('import lockfile from "proper-lockfile";', 'import {lockfile} from "./stubs.mjs";')
    .replace('from "../config.ts";', 'from "./stubs.mjs";')
    .replace('from "../utils/paths.ts";', 'from "./stubs.mjs";')
    .replace('from "../utils/text.ts";', 'from "./text.ts";');
  await writeFile(join(temp,'settings.ts'),rewritten);
  await writeFile(join(temp,'text.ts'),textSource);
  await writeFile(join(temp,'retry.ts'),retrySource);
  await writeFile(join(temp,'http-dispatcher.ts'),httpSource.replace('from "undici";', 'from "./unused-undici.mjs";'));
  await writeFile(join(temp,'unused-undici.mjs'), `
    export const setGlobalDispatcher = () => { throw new Error('Network dispatcher not exercised'); };
    export class EnvHttpProxyAgent { constructor() { throw new Error('Network dispatcher not exercised'); } }
    export class Client extends EnvHttpProxyAgent {};
    export class Pool extends EnvHttpProxyAgent {};
  `);
  await writeFile(join(temp,'stubs.mjs'),`
    const unused = () => {throw new Error('Unexercised dependency called');};
    export {DEFAULT_MAX_AGENT_RETRY_DELAY_MS} from './retry.ts';
    export const DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300000, CONFIG_DIR_NAME = '.pi';
    export const getAgentDir = unused, normalizePath = unused, resolvePath = unused, parseHttpIdleTimeoutMs = unused;
    export const lockfile = {lockSync: unused};
  `);
  const {SettingsManager, InMemorySettingsStorage} = await import(pathToFileURL(join(temp,'settings.ts')));
  const cases = [{name:'absent',global:{}}];
  for (const [name,value] of [
    ['zero',0],['disabled','disabled'],['case-space',' DISABLED '],['fraction',1500.9],
    ['negative-zero','-0'],['hex','0x10'],['octal','0o10'],['binary','0b10'],['scientific','1e3'],
    ['sub-millisecond','.5'],['signed','+17'],['negative-fraction','-.5'],['bom',' \ufeff500 '],
    ['non-js-whitespace','\u0085500\u0085'],['empty',''],['whitespace','   '],['infinity','Infinity'],
    ['nan','NaN'],['text','oops'],['negative',-5],['null',null],['false',false],['array',[]],
    ['object',{}],['true',true],['number-array',[5]],['safe-large',9007199254740991],
  ]) cases.push({name,global:{httpIdleTimeoutMs:value}});
  cases.push({name:'project-null',global:{httpIdleTimeoutMs:1000},project:{httpIdleTimeoutMs:null}});
  cases.push({name:'project-repair',global:{httpIdleTimeoutMs:-5},project:{httpIdleTimeoutMs:2500}});
  cases.push({name:'project-invalid',global:{},project:{httpIdleTimeoutMs:'oops'}});
  const result = settings => {
    try { return {ms:settings.getHttpIdleTimeoutMs()}; }
    catch(e) { return {error:e.message}; }
  };
  for (const c of cases) {
    c.global.defaultProvider='kept-provider';c.project ??= {};
    const storage = new InMemorySettingsStorage();
    storage.withLock('global',()=>JSON.stringify(c.global));storage.withLock('project',()=>JSON.stringify(c.project));
    const settings = SettingsManager.fromStorage(storage);
    c.effective = result(settings);
    c.dispatcher = result(SettingsManager.inMemory(c.global));
    c.defaultProvider = settings.getDefaultProvider();
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,node:process.version,sourceSha256:createHash('sha256').update(source).digest('hex'),textSha256:createHash('sha256').update(textSource).digest('hex'),retrySha256:createHash('sha256').update(retrySource).digest('hex'),dispatcherSha256:createHash('sha256').update(httpSource).digest('hex'),scope:'Actual SettingsManager and original timeout parser; global and project-merged getters; import paths rewritten, unused I/O/network dependencies throw if called',cases},null,2)+'\n');
} finally {await rm(temp,{recursive:true,force:true});}
