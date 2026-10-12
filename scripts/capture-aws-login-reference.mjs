// Execute the pinned AWS login provider with synthetic cache files and no network.
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {createHash,generateKeyPairSync,createPublicKey,verify} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {join,dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {Readable} from 'node:stream';
const [reference,sdkRoot,output]=process.argv.slice(2);
if(!reference||!sdkRoot||!output) throw new Error('Usage: node scripts/capture-aws-login-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin='d2a311097cbcf669e699479587332ae3988a49d0';
if(execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw new Error('Unexpected reference');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/ai/src/api/bedrock-converse-stream.ts']);
const lock=JSON.parse(await readFile(join(reference,'package-lock.json')));
const require=createRequire(join(sdkRoot,'package.json'));
const sources={};
for(const [name,file] of [['credential-provider-login','dist-cjs/index.js'],['nested-clients','dist-cjs/submodules/signin/index.js']]){
 const base=join(sdkRoot,'node_modules/@aws-sdk',name),pkg=JSON.parse(await readFile(join(base,'package.json'))),locked=lock.packages[`node_modules/@aws-sdk/${name}`];
 if(pkg.version!==locked.version)throw new Error('Unexpected SDK version');
 sources[name]={version:pkg.version,integrity:locked.integrity,file,sha256:createHash('sha256').update(await readFile(join(base,file))).digest('hex')};
}
const {privateKey,publicKey}=generateKeyPairSync('ec',{namedCurve:'prime256v1'});
const dpopKey=privateKey.export({type:'sec1',format:'pem'});
const session='arn:aws:iam::123456789012:user/synthetic';
const temp=await mkdtemp(join(tmpdir(),'pipkin-login-reference-'));
const oldDirectory=process.env.AWS_LOGIN_CACHE_DIRECTORY;
try{
 process.env.AWS_LOGIN_CACHE_DIRECTORY=temp;
 const configFilepath=join(temp,'config'),filepath=join(temp,'credentials');
 await writeFile(configFilepath,`[profile test]\nlogin_session=${session}\nregion=us-west-2\n`);
 await writeFile(filepath,'');
 const cacheName=createHash('sha256').update(session).digest('hex')+'.json',cache=join(temp,cacheName);
 const {fromLoginCredentials}=require('@aws-sdk/credential-provider-login');
 const cases=[];
 for(const name of ['valid','refresh','refresh-default-expiry','transient-failure','expired-failure','access-denied']){
  const near=new Date(Date.now()+120000).toISOString();
  const token={accessToken:{accessKeyId:'OLD-ID',secretAccessKey:'old-secret',sessionToken:'old-session',accountId:'123456789012',expiresAt:name==='valid'?'2099-01-01T00:00:00Z':name==='transient-failure'||name==='access-denied'?near:'2000-01-01T00:00:00Z'},clientId:'client',refreshToken:'old-refresh',dpopKey,custom:{keep:true}};
  await writeFile(cache,JSON.stringify(token));
  const requests=[];
  const handler={handle:async req=>{
   const [h,p,s]=req.headers.DPoP.split('.'),header=JSON.parse(Buffer.from(h,'base64url')),payload=JSON.parse(Buffer.from(p,'base64url'));
   const signatureValid=verify('sha256',Buffer.from(h+'.'+p),{key:createPublicKey({key:header.jwk,format:'jwk'}),dsaEncoding:'ieee-p1363'},Buffer.from(s,'base64url'));
   if(!signatureValid)throw new Error('Invalid SDK proof');
   requests.push({method:req.method,path:req.path,host:req.hostname,body:JSON.parse(req.body),header,payload:{htm:payload.htm,htu:payload.htu},signatureValid});
   let body,statusCode=200,headers={'content-type':'application/json'};
   if(name.endsWith('failure')){statusCode=500;body={message:'synthetic failure'};}
   else if(name==='access-denied'){statusCode=403;headers['x-amzn-errortype']='AccessDeniedException';body={error:'TOKEN_EXPIRED',message:'synthetic denial'};}
   else{body={accessToken:{accessKeyId:'NEW-ID',secretAccessKey:'new-secret',sessionToken:'new-session'},refreshToken:'new-refresh',tokenType:'aws',...(name==='refresh-default-expiry'?{}:{expiresIn:3600})};}
   return {response:{statusCode,headers,body:Readable.from([JSON.stringify(body)])}};
  }};
  let credentials,error;
  try{const c=await fromLoginCredentials({profile:'test',configFilepath,filepath,ignoreCache:true,clientConfig:{requestHandler:handler,maxAttempts:1}})();credentials={accessKeyId:c.accessKeyId,secretAccessKey:c.secretAccessKey,sessionToken:c.sessionToken,accountId:c.accountId};}
  catch(e){error=e.message;}
  const saved=JSON.parse(await readFile(cache));
  cases.push({name,token,requests,credentials,error,saved:{accessKeyId:saved.accessToken.accessKeyId,refreshToken:saved.refreshToken,custom:saved.custom}});
 }
 await mkdir(dirname(output),{recursive:true});
 await writeFile(output,JSON.stringify({reference:pin,sources,scope:'Synthetic credentials and recording SDK request handler; no live AWS requests',session,cacheName,dpopKey,publicJwk:publicKey.export({format:'jwk'}),cases},null,2)+'\n');
}finally{
 if(oldDirectory===undefined)delete process.env.AWS_LOGIN_CACHE_DIRECTORY;else process.env.AWS_LOGIN_CACHE_DIRECTORY=oldDirectory;
 await rm(temp,{recursive:true,force:true});
}
