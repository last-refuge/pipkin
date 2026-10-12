// Execute the credential SDK pinned by Pi against synthetic CLI files and a request recorder.
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { Readable } from 'node:stream';
const [reference, sdkRoot, output] = process.argv.slice(2);
if (!reference || !sdkRoot || !output) throw new Error('Usage: node scripts/capture-aws-sso-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected Pi revision');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/ai/src/api/bedrock-converse-stream.ts']);
const lock = JSON.parse(await readFile(join(reference,'package-lock.json')));
const require = createRequire(join(sdkRoot,'package.json'));
const sources = {};
for (const name of ['credential-provider-sso','credential-provider-ini','token-providers','nested-clients']) {
  const base = join(sdkRoot,'node_modules/@aws-sdk',name);
  const packageJson = JSON.parse(await readFile(join(base,'package.json')));
  const locked = lock.packages[`node_modules/@aws-sdk/${name}`];
  if (packageJson.version !== locked.version) throw new Error(`Unexpected ${name} version`);
  sources[name] = {version:packageJson.version,integrity:locked.integrity,sha256:createHash('sha256').update(await readFile(join(base,'dist-cjs/index.js'))).digest('hex')};
  if (name === 'nested-clients') {
    sources[name].submodules = {};
    for (const service of ['sso','sso-oidc']) {
      const file = `dist-cjs/submodules/${service}/index.js`;
      sources[name].submodules[file] = createHash('sha256').update(await readFile(join(base,file))).digest('hex');
    }
  }
}
const temp = await mkdtemp(join(tmpdir(),'pipkin-sso-reference-'));
const oldHome = process.env.HOME;
try {
  process.env.HOME = temp;
  const configPath = join(temp,'.aws/config'), credsPath = join(temp,'.aws/credentials');
  await mkdir(join(temp,'.aws/sso/cache'),{recursive:true});
  await writeFile(configPath,'[profile legacy]\nsso_start_url = https://example.awsapps.com/start\nsso_region = us-west-2\nsso_account_id = 123456789012\nsso_role_name = Developer\n[profile modern]\nsso_session = corp\nsso_account_id = 123456789012\nsso_role_name = Developer\n[sso-session corp]\nsso_start_url = https://example.awsapps.com/start\nsso_region = us-west-2\n[profile mfa]\nrole_arn = arn:aws:iam::123456789012:role/r\nsource_profile = base\nmfa_serial = arn:mfa\n');
  await writeFile(credsPath,'[base]\naws_access_key_id = BASE\naws_secret_access_key = synthetic-base\n');
  const {fromSSO} = require('@aws-sdk/credential-provider-sso');
  const {fromIni} = require('@aws-sdk/credential-provider-ini');
  const cases = [];
  for (const [name,profile,cacheId,expiresAt] of [
    ['legacy','legacy','https://example.awsapps.com/start','2099-01-01T00:00:00Z'],
    ['session-refresh','modern','corp','2000-01-01T00:00:00Z'],
    ['expired-legacy','legacy','https://example.awsapps.com/start','2000-01-01T00:00:00Z'],
  ]) {
    const cacheName = createHash('sha1').update(cacheId).digest('hex')+'.json';
    const path = join(temp,'.aws/sso/cache',cacheName);
    const token = {accessToken:'old-access',expiresAt,clientId:'client',clientSecret:'secret',refreshToken:'old-refresh',custom:{keep:true}};
    await writeFile(path,JSON.stringify(token));
    const requests = [];
    const handler = {handle: async request => {
      requests.push({method:request.method,path:request.path,query:request.query,bearer:request.headers['x-amz-sso_bearer_token']??null,body:request.body?JSON.parse(request.body):null});
      const body = request.path === '/token' ? {accessToken:'new-access',refreshToken:'new-refresh',expiresIn:3600} : {roleCredentials:{accessKeyId:'SSO-ID',secretAccessKey:'sso-secret',sessionToken:'sso-session',expiration:4070908800000}};
      return {response:{statusCode:200,headers:{'content-type':'application/json'},body:Readable.from([JSON.stringify(body)])}};
    }};
    let credentials, error;
    try {
      const result = await fromSSO({profile,configFilepath:configPath,filepath:credsPath,ignoreCache:true,clientConfig:{requestHandler:handler,maxAttempts:1}})();
      credentials = {accessKeyId:result.accessKeyId,secretAccessKey:result.secretAccessKey,sessionToken:result.sessionToken,expiration:result.expiration.toISOString()};
    } catch(e) { error = e.message; }
    const saved = JSON.parse(await readFile(path));
    cases.push({name,cacheId,cacheName,token,requests,credentials,error,saved:{accessToken:saved.accessToken,refreshToken:saved.refreshToken,custom:saved.custom}});
  }
  let mfaError;
  try { await fromIni({profile:'mfa',configFilepath:configPath,filepath:credsPath,ignoreCache:true,roleAssumer:async()=>{throw new Error('Unexpected role assumption');}})(); }
  catch(e) {mfaError=e.message;}
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,sources,scope:'Executed pinned SDK credential providers with synthetic CLI files and SDK HTTP request recorder; no live AWS request',cases,mfaError},null,2)+'\n');
} finally {
  if(oldHome===undefined) delete process.env.HOME; else process.env.HOME=oldHome;
  await rm(temp,{recursive:true,force:true});
}
