// Run the pinned Google certificate ADC client against a real local mutual-TLS endpoint.
import {readFile, writeFile, mkdir, mkdtemp, rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {createServer} from 'node:https';
import {join, dirname, resolve} from 'node:path';
import {tmpdir} from 'node:os';
const [reference, sdkRoot, output] = process.argv.slice(2);
if (!reference || !sdkRoot || !output) throw Error('Usage: node capture-google-certificate-reference.mjs PI_CHECKOUT SDK_ROOT OUTPUT');
const fixture = resolve('fixtures/native-google-certificate');
if (process.env.NODE_EXTRA_CA_CERTS !== join(fixture, 'ca.pem')) throw Error('Launch with NODE_EXTRA_CA_CERTS pointing to the synthetic fixture CA');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw Error('Unexpected reference');
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', 'package-lock.json', 'packages/ai/src/api/google-vertex.ts']);
const lock = JSON.parse(await readFile(join(reference, 'package-lock.json')));
const base = join(sdkRoot, 'node_modules/google-auth-library');
const pkg = JSON.parse(await readFile(join(base, 'package.json')));
const locked = lock.packages['node_modules/google-auth-library'];
if (pkg.version !== locked.version) throw Error('Unexpected Google SDK');
const hashes = {};
for (const name of ['certificatesubjecttokensupplier', 'identitypoolclient', 'baseexternalclient', 'impersonated']) {
  hashes[name] = createHash('sha256').update(await readFile(join(base, `build/src/auth/${name}.js`))).digest('hex');
}
hashes.genaiNode = createHash('sha256').update(await readFile(join(sdkRoot, 'node_modules/@google/genai/dist/node/index.mjs'))).digest('hex');
const require = createRequire(join(sdkRoot, 'package.json'));
const {GoogleAuth} = require('google-auth-library');
const keys = ['GOOGLE_API_CERTIFICATE_CONFIG','CLOUDSDK_CONFIG','HOME','APPDATA'];
const previous = Object.fromEntries(keys.map(k => [k, process.env[k]]));
const serverOptions = {key:await readFile(join(fixture,'server-key.pem')), cert:await readFile(join(fixture,'server.pem')), ca:await readFile(join(fixture,'ca.pem')), requestCert:true, rejectUnauthorized:true};
let requests = [];
const server = createServer(serverOptions, async (req,res) => {
  let bytes=''; for await (const chunk of req) bytes += chunk;
  const body=req.headers['content-type']?.includes('json') ? JSON.parse(bytes) : Object.fromEntries(new URLSearchParams(bytes));
  requests.push({method:req.method, path:req.url, authorization:req.headers.authorization??null, body, certificate:req.socket.getPeerCertificate().fingerprint256});
  res.setHeader('content-type','application/json');
  res.end(JSON.stringify(req.url === '/token' ? {access_token:'certificate-token', expires_in:3600} : {accessToken:'impersonated-token', expireTime:'2099-01-01T00:00:00Z'}));
});
server.on('tlsClientError', () => {});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const tls = `https://127.0.0.1:${server.address().port}`;
const temp = await mkdtemp(join(tmpdir(),'pipkin-certificate-reference-'));
try {
  const cases=[];
  for (const name of ['leaf','trust-prepend','trust-leaf-first','trust-leaf-later','trust-empty','trust-invalid','env','cloudsdk','home','invalid-explicit','invalid-env','both','malformed-config','missing-key','invalid-cert','invalid-key','mismatched-key','impersonation','nested-impersonation']) {
    const dir=join(temp,name); await mkdir(dir);
    const files={'config.json':JSON.stringify({cert_configs:{workload:{cert_path:'$FIXTURE/client.pem',key_path:'$FIXTURE/client-key.pem'}}})};
    const env={HOME:'$TEMP/home',APPDATA:'$TEMP/appdata'};
    const certificate={certificate_config_location:'$TEMP/config.json'};
    if (name.startsWith('trust-')) {
      certificate.trust_chain_path='$TEMP/chain.pem';
      const ca=await readFile(join(fixture,'ca.pem'),'utf8'),leaf=await readFile(join(fixture,'client.pem'),'utf8');
      files['chain.pem']=name==='trust-prepend'?ca:name==='trust-leaf-first'?leaf+ca:name==='trust-leaf-later'?ca+leaf:name==='trust-empty'?'no certificate blocks':'-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----\n';
    }
    if (['env','cloudsdk','home','invalid-env'].includes(name)) {
      delete certificate.certificate_config_location;certificate.use_default_certificate_config=true;
      if (name==='env') env.GOOGLE_API_CERTIFICATE_CONFIG='$TEMP/config.json';
      if (name==='cloudsdk') {env.CLOUDSDK_CONFIG='$TEMP/sdk';files['sdk/certificate_config.json']=files['config.json'];}
      if (name==='home'||name==='invalid-env') files['home/.config/gcloud/certificate_config.json']=files['config.json'];
      if (name==='invalid-env') env.GOOGLE_API_CERTIFICATE_CONFIG='$TEMP/missing';
    }
    if (name==='invalid-explicit') {certificate.certificate_config_location='$TEMP/missing';env.GOOGLE_API_CERTIFICATE_CONFIG='$TEMP/config.json';}
    if (name==='both') certificate.use_default_certificate_config=true;
    if (name==='malformed-config') files['config.json']='secret malformed JSON';
    if (name==='missing-key') files['config.json']=JSON.stringify({cert_configs:{workload:{cert_path:'$FIXTURE/client.pem'}}});
    if (name==='invalid-cert'||name==='invalid-key') {
      files['bad']='secret malformed identity';
      files['config.json']=files['config.json'].replace(name==='invalid-cert'?'$FIXTURE/client.pem':'$FIXTURE/client-key.pem','$TEMP/bad');
    }
    if (name==='mismatched-key') files['config.json']=files['config.json'].replace('client-key.pem','rotated-key.pem');
    let doc={type:'external_account',audience:'//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/provider',subject_token_type:'urn:ietf:params:oauth:token-type:mtls',token_url:'$TLS/token',credential_source:{certificate},quota_project_id:'billing'};
    if (name==='impersonation') doc.service_account_impersonation_url='$TLS/v1/projects/-/serviceAccounts/target@example:generateAccessToken';
    if (name==='nested-impersonation') doc={type:'impersonated_service_account',source_credentials:doc,service_account_impersonation_url:'$TLS/v1/projects/-/serviceAccounts/target@example:generateAccessToken',endpoint:'$TLS',quota_project_id:'billing'};
    const expand=s=>s.replaceAll('$TEMP',dir).replaceAll('$FIXTURE',fixture).replaceAll('$TLS',tls);
    for(const [path,content] of Object.entries(files)){await mkdir(dirname(join(dir,path)),{recursive:true});await writeFile(join(dir,path),expand(content));}
    for(const key of keys){delete process.env[key];if(env[key]!==undefined)process.env[key]=expand(env[key]);}
    requests=[];let outcome;
    try {const client=new GoogleAuth({scopes:['https://www.googleapis.com/auth/cloud-platform']}).fromJSON(JSON.parse(expand(JSON.stringify(doc))));outcome={token:(await client.getAccessToken()).token};}
    catch(error){outcome={error:true,errorType:error.constructor.name};}
    cases.push({name,document:doc,environment:env,files,outcome,requests});
    server.closeIdleConnections();
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,version:pkg.version,integrity:locked.integrity,hashes,scope:'Executed GoogleAuth certificate ADC and original STS/IAM transports against a local server requiring mutual TLS; synthetic keys only',cases},null,2)+'\n');
} finally {
  server.closeAllConnections();await new Promise(r=>server.close(r));await rm(temp,{recursive:true,force:true});
  for(const key of keys){if(previous[key]===undefined)delete process.env[key];else process.env[key]=previous[key];}
}
