// Execute Node TLS and Pi's pinned undici with a startup-only extra CA environment.
import { readFile, writeFile, mkdtemp, rm, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { createServer } from 'node:https';
import { join, dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
const [reference,sdkRoot,output]=process.argv.slice(2);
if(!reference||!sdkRoot||!output)throw new Error('Usage: node scripts/capture-extra-ca-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin='d2a311097cbcf669e699479587332ae3988a49d0';
if(execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw new Error('Unexpected Pi revision');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/coding-agent/src/core/http-dispatcher.ts']);
const lock=JSON.parse(await readFile(join(reference,'package-lock.json')));
const pkg=JSON.parse(await readFile(join(sdkRoot,'node_modules/undici/package.json')));
if(pkg.version!==lock.packages['node_modules/undici'].version)throw new Error('Unexpected undici version');
const hash=async path=>createHash('sha256').update(await readFile(path)).digest('hex');
const temp=await mkdtemp(join(tmpdir(),'pipkin-extra-ca-reference-'));
const ca=await readFile('fixtures/native-google-certificate/ca.pem','utf8');
const server=createServer({cert:await readFile('fixtures/native-google-certificate/server.pem'),key:await readFile('fixtures/native-google-certificate/server-key.pem')},(_,res)=>res.end('verified'));
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const url=`https://localhost:${server.address().port}`;
const cases=[];
try {
  const sources=[['unset',null],['valid',ca],['duplicate',ca+ca],['text-prefix','ignored text\n'+ca],['empty',''],['garbage','not a certificate'],['malformed','-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n'],['valid-then-malformed',ca+'-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n'],['malformed-then-valid','-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n'+ca],['missing',null]];
  const script=`const tls=require('node:tls'); const {fetch,Agent}=require(${JSON.stringify(resolve(sdkRoot,'node_modules/undici'))}); fetch(process.argv[1],{dispatcher:new Agent({allowH2:false})}).then(async r=>console.log(JSON.stringify({ok:true,body:await r.text(),roots:tls.getCACertificates('extra').length}))).catch(e=>console.log(JSON.stringify({ok:false,error:e.cause?.code??e.message,roots:tls.getCACertificates('extra').length})));`;
  for(const [name,pem] of sources){
    const path=join(temp,name+'.pem');if(pem!==null)await writeFile(path,pem);
    const env={PATH:process.env.PATH,HOME:temp};if(name!=='unset')env.NODE_EXTRA_CA_CERTS=path;
    const result=await new Promise((resolve,reject)=>{
      const child=spawn(process.execPath,['-e',script,url],{env,stdio:['ignore','pipe','pipe']});let stdout='',stderr='';
      child.stdout.on('data',x=>stdout+=x);child.stderr.on('data',x=>stderr+=x);child.on('error',reject);
      child.on('close',code=>code===0?resolve({out:JSON.parse(stdout),warning:stderr.includes('Ignoring extra certs')}):reject(new Error(stderr)));
    });
    cases.push({name,...result.out,warning:result.warning});
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,node:process.version,undici:{version:pkg.version,integrity:lock.packages['node_modules/undici'].integrity,entrySha256:await hash(join(sdkRoot,'node_modules/undici/index.js'))},dispatcherSha256:await hash(join(reference,'packages/coding-agent/src/core/http-dispatcher.ts')),scope:'Real local TLS requests through pinned undici with isolated child startup environments',cases},null,2)+'\n');
}finally{server.close();server.closeAllConnections();await rm(temp,{recursive:true,force:true});}
