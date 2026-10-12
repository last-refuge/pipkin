// Execute pinned external-account clients with temporary synthetic tokens and a recording transporter.
import {readFile,writeFile,mkdir,mkdtemp,rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {join,dirname} from 'node:path';
import {tmpdir} from 'node:os';
const [reference,sdkRoot,output]=process.argv.slice(2);
if(!reference||!sdkRoot||!output)throw new Error('Usage: node scripts/capture-google-external-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin='d2a311097cbcf669e699479587332ae3988a49d0';
if(execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw new Error('Unexpected reference');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/ai/src/api/google-vertex.ts']);
const lock=JSON.parse(await readFile(join(reference,'package-lock.json'))),base=join(sdkRoot,'node_modules/google-auth-library'),pkg=JSON.parse(await readFile(join(base,'package.json'))),locked=lock.packages['node_modules/google-auth-library'];
if(pkg.version!==locked.version)throw new Error('Unexpected Google SDK version');
const hashes={};for(const file of ['baseexternalclient','identitypoolclient','filesubjecttokensupplier','urlsubjecttokensupplier','stscredentials','oauth2common','externalAccountAuthorizedUserClient']){
 hashes[file]=createHash('sha256').update(await readFile(join(base,`build/src/auth/${file}.js`))).digest('hex');
}
const require=createRequire(join(sdkRoot,'package.json'));const {GoogleAuth}=require('google-auth-library');
const scope='https://www.googleapis.com/auth/cloud-platform';
const temp=await mkdtemp(join(tmpdir(),'pipkin-external-reference-'));
try{
 const cases=[];
 for(const name of ['file-text','file-json','url-json','workforce','workforce-basic','impersonation','no-expiry','external-user']){
  const doc={type:'external_account',audience:'//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/provider',subject_token_type:'urn:ietf:params:oauth:token-type:jwt',token_url:'https://sts.example/v1/token',credential_source:{file:join(temp,'subject')},quota_project_id:'billing'};
  let subject='raw-token\n';
  if(name==='file-json'){subject=JSON.stringify({jwt:'json-token',ignored:'x'});doc.credential_source.format={type:'json',subject_token_field_name:'jwt'};}
  if(name==='url-json')doc.credential_source={url:'https://issuer.example/subject',headers:{'X-Identity':'selector'},format:{type:'json',subject_token_field_name:'jwt'}};
  if(name.startsWith('workforce')){doc.audience='//iam.googleapis.com/locations/global/workforcePools/pool/providers/provider';doc.workforce_pool_user_project='workforce-billing';}
  if(name==='workforce-basic'){doc.client_id='client';doc.client_secret='secret';}
  if(name==='impersonation'){doc.service_account_impersonation_url='https://iam.example/v1/projects/-/serviceAccounts/target@example:generateAccessToken';doc.service_account_impersonation={token_lifetime_seconds:1800};}
  if(name==='external-user'){for(const key of Object.keys(doc))delete doc[key];Object.assign(doc,{type:'external_account_authorized_user',client_id:'client',client_secret:'secret',refresh_token:'initial-refresh',token_url:'https://sts.example/v1/oauthtoken',quota_project_id:'billing'});}
  await writeFile(join(temp,'subject'),subject);
  const client=new GoogleAuth({scopes:[scope]}).fromJSON(doc),requests=[];
  let refresh=0;
  const send=async opts=>{
   const headers=new Headers(opts.headers),body=opts.data instanceof URLSearchParams?Object.fromEntries(opts.data):opts.data??null;
   requests.push({method:opts.method,url:String(opts.url),authorization:headers.get('authorization'),identity:headers.get('x-identity'),body});
   let data;
   if(String(opts.url).includes('/subject'))data={jwt:'url-token'};
   else if(String(opts.url).includes('generateAccessToken'))data={accessToken:'impersonated-token',expireTime:'2099-01-01T00:00:00Z'};
   else data={access_token:'federated-token',...(name==='no-expiry'?{}:{expires_in:3600}),...(name==='external-user'?{refresh_token:`rotated-${++refresh}`}:{})};
   return {data,status:200,statusText:'OK',headers:new Headers(),config:opts};
  };
  client.transporter.request=send;
  if(client.stsCredential)client.stsCredential.transporter.request=send;
  const tokens=[(await client.getAccessToken()).token];
  if(name==='external-user'){client.cachedAccessToken.expiry_date=1;tokens.push((await client.getAccessToken()).token);}
  if(doc.credential_source?.file)doc.credential_source.file='$SUBJECT_FILE';
  cases.push({name,document:doc,subject,requests,tokens,quota:client.quotaProjectId});
 }
 await mkdir(dirname(output),{recursive:true});await writeFile(output,JSON.stringify({reference:pin,version:pkg.version,integrity:locked.integrity,hashes,scope:'Executed GoogleAuth external-account clients with synthetic files and transporter; no live account',cases},null,2)+'\n');
}finally{await rm(temp,{recursive:true,force:true});}
