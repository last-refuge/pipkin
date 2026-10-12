// Run the original SDK pinned by Pi against deterministic token/file/clock fixtures.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname, resolve} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';

const [pi, sdkPath, output] = process.argv.slice(2);
if (!pi || !sdkPath || !output) throw new Error('Usage: node scripts/capture-anthropic-federation-reference.mjs PI_SOURCE SDK_DIR OUTPUT');
const sdk = resolve(sdkPath);
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C',pi,'rev-parse','HEAD'], {encoding:'utf8'}).trim() !== reference) throw new Error('Wrong Pi revision');
execFileSync('git', ['-C',pi,'diff','--exit-code','HEAD','--','package-lock.json']);
const lock = JSON.parse(await readFile(join(pi,'package-lock.json'),'utf8'));
const expected = lock.packages['node_modules/@anthropic-ai/sdk'];
const pkg = JSON.parse(await readFile(join(sdk,'package.json'),'utf8'));
if (expected.version !== '0.129.0' || pkg.version !== expected.version) throw new Error('Wrong SDK version');
const sources = {};
for (const file of ['oidc-federation','identity-token','token-cache','types']) {
  for (const [dir, ext] of [['src/lib/credentials','ts'],['lib/credentials','mjs']]) {
    const path = `${dir}/${file}.${ext}`;
    sources[path] = createHash('sha256').update(await readFile(join(sdk,path))).digest('hex');
  }
}
const {oidcFederationProvider} = await import(pathToFileURL(join(sdk,'lib/credentials/oidc-federation.mjs')));
const {identityTokenFromFile} = await import(pathToFileURL(join(sdk,'lib/credentials/identity-token.mjs')));
const {TokenCache} = await import(pathToFileURL(join(sdk,'lib/credentials/token-cache.mjs')));
const temp = await mkdtemp(join(tmpdir(),'pipkin-federation-reference-'));
const originalNow = Date.now;
try {
  let now = 1700000000000;
  Date.now = () => now;
  const file = join(temp,'identity');
  await writeFile(file, '  assertion-one\n');
  const wire = [];
  for (const extras of [{}, {workspaceId:'workspace-a',serviceAccountId:'service'}]) {
    const provider = oidcFederationProvider({baseURL:'http://127.0.0.1:1234',
      identityTokenProvider:identityTokenFromFile(file), federationRuleId:'rule', organizationId:'org', ...extras,
      fetch:async (url, init) => {
        wire.push({url, method:init.method, headers:init.headers, body:JSON.parse(init.body)});
        return new Response(JSON.stringify({access_token:'captured-token',expires_in:3600,token_type:'Bearer'}),{status:200});
      }});
    const token = await provider();
    wire.at(-1).token = token;
  }
  let calls=0;
  const cache = new TokenCache(async () => ({token:`token-${++calls}`,expiresAt:now/1000+3600}));
  const cacheCases = [];
  const take = async label => cacheCases.push({label,token:await cache.getToken(),calls});
  await take('initial');
  await take('cached');
  now += 3500*1000; // advisory window
  await take('advisory');
  await Promise.resolve();
  await take('after-advisory');
  cache.invalidate();
  await take('invalidated');
  now += 3590*1000; // mandatory window
  await take('mandatory');
  const report = {reference,sdkVersion:pkg.version,sdkIntegrity:expected.integrity,sources,
    scope:'Original SDK federation provider, file source and TokenCache; fake fetch and clock, no live credentials or provider calls',wire,cacheCases};
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify(report,null,2)+'\n');
  console.log(`Captured ${wire.length} exchanges and ${cacheCases.length} cache transitions`);
} finally { Date.now = originalNow; await rm(temp,{recursive:true,force:true}); }
