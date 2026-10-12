// Execute Google's pinned AWS federation client with synthetic credentials and recorded transport.
import {readFile, writeFile, mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {join, dirname} from 'node:path';
const [reference, sdkRoot, output] = process.argv.slice(2);
if (!reference || !sdkRoot || !output) throw Error('Usage: node capture-google-aws-reference.mjs PI_CHECKOUT SDK_ROOT OUTPUT');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw Error('Unexpected reference');
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', 'package-lock.json', 'packages/ai/src/api/google-vertex.ts']);
const lock = JSON.parse(await readFile(join(reference, 'package-lock.json')));
const base = join(sdkRoot, 'node_modules/google-auth-library');
const pkg = JSON.parse(await readFile(join(base, 'package.json')));
const locked = lock.packages['node_modules/google-auth-library'];
if (pkg.version !== locked.version) throw Error('Unexpected Google SDK');
const hashes = {};
for (const name of ['awsclient', 'awsrequestsigner', 'defaultawssecuritycredentialssupplier', 'baseexternalclient', 'externalclient']) {
  hashes[name] = createHash('sha256').update(await readFile(join(base, `build/src/auth/${name}.js`))).digest('hex');
}
const require = createRequire(join(sdkRoot, 'package.json'));
const {GoogleAuth} = require('google-auth-library');
const OriginalDate = Date, timestamp = '2026-10-12T06:07:08.000Z';
globalThis.Date = class extends OriginalDate {
  constructor(...args) { super(...(args.length ? args : [timestamp])); }
  static now() { return OriginalDate.parse(timestamp); }
};
const keys = ['AWS_REGION','AWS_DEFAULT_REGION','AWS_ACCESS_KEY_ID','AWS_SECRET_ACCESS_KEY','AWS_SESSION_TOKEN'];
const prior = Object.fromEntries(keys.map(k => [k, process.env[k]]));
try {
  const cases = [];
  for (const name of ['environment', 'session', 'default-region', 'metadata-v1', 'metadata-v2', 'partial-environment', 'env-region', 'env-credentials', 'camel-options', 'camel-fields', 'raw-query', 'missing-region', 'unsupported-version']) {
    const env = {AWS_REGION:'us-east-2', AWS_ACCESS_KEY_ID:'ENVKEY', AWS_SECRET_ACCESS_KEY:'env-secret'};
    const source = {environment_id:'aws1', region_url:'http://metadata.example/region', url:'http://metadata.example/role', regional_cred_verification_url:'https://sts.{region}.amazonaws.com?Action=GetCallerIdentity&Version=2011-06-15'};
    if (name === 'session') env.AWS_SESSION_TOKEN = 'env-session+/=';
    if (name === 'default-region') { delete env.AWS_REGION; env.AWS_DEFAULT_REGION = 'eu-west-1'; }
    if (name.startsWith('metadata') || name.startsWith('camel-')) { for (const key of keys) delete env[key]; }
    if (name === 'metadata-v2' || name === 'partial-environment' || name === 'env-region' || name === 'env-credentials') source.imdsv2_session_token_url = 'http://metadata.example/imds';
    if (name === 'partial-environment' || name === 'env-region') delete env.AWS_SECRET_ACCESS_KEY;
    if (name === 'env-credentials') delete env.AWS_REGION;
    if (name === 'raw-query') source.regional_cred_verification_url = 'https://sts.{region}.amazonaws.com:8443/a%2Fb?Version=2011-06-15&Action=GetCallerIdentity&x=%2F&x=second';
    if (name === 'missing-region') { delete env.AWS_REGION; delete source.region_url; }
    if (name === 'unsupported-version') source.environment_id = 'aws2';
    for (const key of keys) { delete process.env[key]; if (env[key] !== undefined) process.env[key] = env[key]; }
    let doc = {type:'external_account', audience:'//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/provider', subject_token_type:'urn:ietf:params:aws:token-type:aws4_request', token_url:'https://sts.example/v1/token', credential_source:source, quota_project_id:'billing'};
    if (name === 'camel-options') doc = {type:doc.type, audience:doc.audience, subjectTokenType:doc.subject_token_type, tokenUrl:doc.token_url, quotaProjectId:'billing', credentialSource:{environmentId:'aws01', regionUrl:source.region_url, url:source.url, regionalCredVerificationUrl:source.regional_cred_verification_url}};
    if (name === 'camel-fields') doc.credential_source = {environment_id:'aws01', regionUrl:source.region_url, url:source.url, regionalCredVerificationUrl:source.regional_cred_verification_url};
    const requests = [], tokens = [], subjects = [];
    let imds = 0, roles = 0, creds = 0, regions = 0, outcome;
    try {
      const client = new GoogleAuth({scopes:['https://www.googleapis.com/auth/cloud-platform']}).fromJSON(doc);
      const send = async opts => {
        const url = String(opts.url), headers = new Headers(opts.headers);
        const body = opts.data instanceof URLSearchParams ? Object.fromEntries(opts.data) : opts.data ?? null;
        requests.push({method:opts.method ?? 'GET', url, metadataToken:headers.get('x-aws-ec2-metadata-token'), ttl:headers.get('x-aws-ec2-metadata-token-ttl-seconds'), body});
        let data;
        if (url.endsWith('/imds')) data = `metadata-token-${++imds}`;
        else if (url.endsWith('/region')) data = ++regions === 1 ? 'ap-southeast-2a' : 'unwanted-second-region-a';
        else if (url.endsWith('/role')) data = `role-${++roles}`;
        else if (url.includes('/role/')) data = {AccessKeyId:`METAKEY${++creds}`, SecretAccessKey:'meta-secret', Token:`meta-session-${creds}`};
        else { subjects.push(body.subject_token); data = {access_token:'federated', expires_in:3600}; }
        return {data, status:200, statusText:'OK', headers:new Headers(), config:opts};
      };
      client.transporter.request = send;
      client.stsCredential.transporter.request = send;
      tokens.push((await client.getAccessToken()).token);
      client.cachedAccessToken.expiry_date = 1;
      tokens.push((await client.getAccessToken()).token);
      outcome = {tokens};
    } catch (error) { outcome = {error:true, errorType:error.constructor.name}; }
    cases.push({name, document:doc, environment:env, requests, subjects, outcome});
  }
  await mkdir(dirname(output), {recursive:true});
  await writeFile(output, JSON.stringify({reference:pin, version:pkg.version, integrity:locked.integrity, hashes, timestamp, scope:'Executed Google AWS ADC client, original signer and synthetic recording transport; no live AWS/Google requests', cases}, null, 2) + '\n');
} finally {
  globalThis.Date = OriginalDate;
  for (const key of keys) { if (prior[key] === undefined) delete process.env[key]; else process.env[key] = prior[key]; }
}
