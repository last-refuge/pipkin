// Execute the original bounded Copilot retry helper with deterministic fetch/sleep/clock.
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join,dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root,output]=process.argv.slice(2);
if(!root||!output) throw new Error('Usage: node scripts/capture-copilot-retry-reference.mjs PI_SOURCE OUTPUT_JSON');
process.env.TZ='UTC';
const reference='d2a311097cbcf669e699479587332ae3988a49d0';
const path='packages/ai/src/auth/oauth/github-copilot.ts';
if(execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==reference) throw new Error('Wrong reference');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',path]);
const source=await readFile(join(root,path),'utf8');
const start=source.indexOf('async function fetchWithRateLimitRetry(');
const end=source.indexOf('\nasync function fetchGitHubCopilotModels(',start);
if(start<0||end<0) throw new Error('Helper not found');
const dir=await mkdtemp(join(tmpdir(),'pipkin-copilot-retry-'));
const realNow=Date.now, realFetch=globalThis.fetch;
try {
 await writeFile(join(dir,'retry.ts'),'export '+source.slice(start,end)+'\nasync function sleep(delay,signal) { globalThis.recordSleep(delay,signal); }\n');
 const {fetchWithRateLimitRetry}=await import(pathToFileURL(join(dir,'retry.ts')));
 const nowMs=Date.parse('2026-10-11T12:00:00Z');
 const headers=[null,'','0','-1','0.125','+.5','1e-2','0 trailing','0x20','1e','Infinity','-Infinity','NaN','1e999','1e250','5000','not a date','Sun, 11 Oct 2026 12:00:01 GMT','Sun, 11 Oct 2026 11:59:00 GMT','Sunday, 11-Oct-26 12:00:01 GMT','Sun Oct 11 12:00:01 2026'];
 const cases=[];
 for(const header of headers) {
  let now=nowMs, requests=0; const delays=[];
  Date.now=()=>now;
  globalThis.recordSleep=delay=>{delays.push(delay);now+=delay;};
  globalThis.fetch=async()=>{requests++;return new Response('limited',{status:429,headers:header===null?{}:{'retry-after':header}});};
  const response=await fetchWithRateLimitRetry('https://fixture.invalid',{},new AbortController().signal,{maxRetries:2,maxElapsedMs:5000});
  cases.push({header,requests,delays,status:response.status});
 }
 await mkdir(dirname(output),{recursive:true});
 await writeFile(output,JSON.stringify({reference,sources:{[path]:createHash('sha256').update(source).digest('hex')},nowMs,timeZone:'UTC',scope:'Original retry helper with fake fetch, clock and sleep; elapsed timeout/cancellation require native real-time tests.',cases},null,2)+'\n');
} finally {Date.now=realNow;globalThis.fetch=realFetch;delete globalThis.recordSleep;await rm(dir,{recursive:true,force:true});}
