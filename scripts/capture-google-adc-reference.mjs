// Execute pinned Google auth constructors against a synthetic transporter and metadata supplier.
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {join,dirname} from 'node:path';
const [reference,sdkRoot,output]=process.argv.slice(2);
if(!reference||!sdkRoot||!output)throw new Error('Usage: node scripts/capture-google-adc-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin='d2a311097cbcf669e699479587332ae3988a49d0';
if(execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw new Error('Unexpected reference');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/ai/src/api/google-vertex.ts']);
const lock=JSON.parse(await readFile(join(reference,'package-lock.json')));
const require=createRequire(join(sdkRoot,'package.json'));
const sources={};
for(const [name,files] of [['google-auth-library',['build/src/auth/googleauth.js','build/src/auth/impersonated.js','build/src/auth/computeclient.js','build/src/auth/refreshclient.js']],['gcp-metadata',['build/src/index.js']]]){
 const base=join(sdkRoot,'node_modules',name),pkg=JSON.parse(await readFile(join(base,'package.json'))),locked=lock.packages[`node_modules/${name}`];
 if(pkg.version!==locked.version)throw new Error(`Unexpected ${name} version`);
 const hashes={};for(const file of files)hashes[file]=createHash('sha256').update(await readFile(join(base,file))).digest('hex');
 sources[name]={version:pkg.version,integrity:locked.integrity,hashes};
}
const {GoogleAuth,Compute}=require('google-auth-library');
const scope='https://www.googleapis.com/auth/cloud-platform';
const cases=[];
for(const delegates of [[],['delegate@project.iam.gserviceaccount.com']]){
 const doc={type:'impersonated_service_account',service_account_impersonation_url:'https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/target@project.iam.gserviceaccount.com:generateAccessToken',source_credentials:{type:'authorized_user',client_id:'client',client_secret:'secret',refresh_token:'refresh',quota_project_id:'source-billing'},delegates,quota_project_id:'target-billing',lifetime:1800};
 const client=new GoogleAuth({scopes:[scope]}).fromJSON(doc),requests=[];
 const send=async opts=>{
  const headers=new Headers(opts.headers),body=opts.data instanceof URLSearchParams?Object.fromEntries(opts.data):opts.data;
  requests.push({method:opts.method,url:String(opts.url),authorization:headers.get('authorization'),quota:headers.get('x-goog-user-project'),body});
  const data=String(opts.url).includes('generateAccessToken')?{accessToken:'impersonated-token',expireTime:'2099-01-01T00:00:00Z'}:{access_token:'source-token',expires_in:3600,token_type:'Bearer'};
  return {data,status:200,statusText:'OK',headers:new Headers(),config:opts};
 };
 client.transporter.request=send;client.sourceClient.transporter.request=send;
 const token=await client.getAccessToken();
 cases.push({document:doc,requests,token:token.token,quota:client.quotaProjectId});
}
const metadata=require('gcp-metadata');const old=metadata.instance;
const calls=[];
try{metadata.instance=async opts=>{calls.push(opts);return {access_token:'metadata-token',expires_in:3600,token_type:'Bearer'};};
 const c=new Compute({scopes:[scope]});const token=await c.getAccessToken();
 cases.push({name:'metadata',calls,token:token.token});
}finally{metadata.instance=old;}
let universeError;
try{new GoogleAuth({scopes:[scope]}).fromJSON({...cases[0].document,universe_domain:'different.example'});}catch(e){universeError=e.message;}
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference:pin,sources,scope:'Executed SDK impersonation with synthetic transporter; Compute with synthetic metadata supplier, not live GCE discovery',cases,universeError},null,2)+'\n');
