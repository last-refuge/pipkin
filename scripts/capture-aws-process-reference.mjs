// Execute the credential SDK pinned by Pi against synthetic CLI files and a request recorder.
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
const [reference, sdkRoot, output] = process.argv.slice(2);
if (!reference || !sdkRoot || !output) throw new Error('Usage: node scripts/capture-aws-process-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected Pi revision');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/ai/src/api/bedrock-converse-stream.ts']);
const lock = JSON.parse(await readFile(join(reference,'package-lock.json')));
const require = createRequire(join(sdkRoot,'package.json'));
const sources = {};
for (const name of ['credential-provider-process']) {
  const base = join(sdkRoot,'node_modules/@aws-sdk',name);
  const packageJson = JSON.parse(await readFile(join(base,'package.json')));
  const locked = lock.packages[`node_modules/@aws-sdk/${name}`];
  if (packageJson.version !== locked.version) throw new Error(`Unexpected ${name} version`);
  sources[name] = {version:packageJson.version,integrity:locked.integrity,sha256:createHash('sha256').update(await readFile(join(base,'dist-cjs/index.js'))).digest('hex')};
}
const temp = await mkdtemp(join(tmpdir(),'pipkin-process-reference-'));
const quote = x => "'" + x.replaceAll("'", "'\\''") + "'";
const valid = {Version:1,AccessKeyId:'PROCESS',SecretAccessKey:'synthetic',SessionToken:'session',Expiration:'2099-01-01T00:00:00Z'};
const printf = value => `printf '%s' ${quote(JSON.stringify(value))}`;
try {
  const {fromProcess} = require('@aws-sdk/credential-provider-process');
  const cases = [];
  for (const [name,command] of [
    ['valid',printf(valid)],
    ['permanent',printf({Version:1,AccessKeyId:'PERMANENT',SecretAccessKey:'synthetic'})],
    ['whitespace',`printf ' \\n'; ${printf(valid)}; printf '\\n'`],
    ['stderr-success',`${printf(valid)}; printf synthetic-detail >&2`],
    ['shell-expansion',`PIPKIN_HELPER_ID=SCOPED; printf '{"Version":1,"AccessKeyId":"%s","SecretAccessKey":"synthetic"}' "$PIPKIN_HELPER_ID"`],
    ['expired',printf({...valid,Expiration:'2000-01-01T00:00:00Z'})],
    ['invalid-version',printf({...valid,Version:2})],
    ['missing-keys',printf({Version:1})],
    ['invalid-json','printf synthetic-detail'],
    ['exit-error','printf synthetic-detail >&2; exit 7'],
    ['stdout-bound','/usr/bin/head -c 1048577 /dev/zero'],
    ['stderr-bound','/usr/bin/head -c 1048577 /dev/zero >&2'],
  ]) {
    const configPath=join(temp,name+'.config'),filepath=join(temp,'empty');
    await writeFile(filepath,'');await writeFile(configPath,`[profile test]\ncredential_process = ${command}\n`);
    let credentials,error;
    try {
      const c=await fromProcess({profile:'test',configFilepath:configPath,filepath,ignoreCache:true})();
      credentials={accessKeyId:c.accessKeyId,secretAccessKey:c.secretAccessKey,sessionToken:c.sessionToken??null,expiration:c.expiration?.toISOString()??null};
    } catch(e) {error=e.message;}
    cases.push({name,command,credentials,error});
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,sources,scope:'Executed pinned SDK credential_process using synthetic shell helpers; no live AWS request',cases},null,2)+'\n');
} finally {await rm(temp,{recursive:true,force:true});}
