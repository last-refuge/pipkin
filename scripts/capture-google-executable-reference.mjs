// Run the pinned Google ADC helper client using synthetic local executable responses.
import {readFile, writeFile, mkdir, mkdtemp, rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
const [reference, sdkRoot, output] = process.argv.slice(2);
if (!reference || !sdkRoot || !output) throw Error('Usage: node capture-google-executable-reference.mjs PI_CHECKOUT SDK_ROOT OUTPUT');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw Error('Unexpected reference');
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', 'package-lock.json', 'packages/ai/src/api/google-vertex.ts']);
const lock = JSON.parse(await readFile(join(reference, 'package-lock.json')));
const base = join(sdkRoot, 'node_modules/google-auth-library');
const pkg = JSON.parse(await readFile(join(base, 'package.json')));
const locked = lock.packages['node_modules/google-auth-library'];
if (pkg.version !== locked.version) throw Error('Unexpected Google SDK');
const hashes = {};
for (const name of ['pluggable-auth-client', 'pluggable-auth-handler', 'executable-response', 'baseexternalclient']) {
  hashes[name] = createHash('sha256').update(await readFile(join(base, `build/src/auth/${name}.js`))).digest('hex');
}
const require = createRequire(join(sdkRoot, 'package.json'));
const {GoogleAuth} = require('google-auth-library');
const {PluggableAuthHandler} = require(join(base, 'build/src/auth/pluggable-auth-handler.js'));
const temp = await mkdtemp(join(tmpdir(), 'pipkin-google-helper-'));
const previous = process.env.GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES;
try {
  const script = `printf '%s\\n' "$GOOGLE_EXTERNAL_ACCOUNT_AUDIENCE" "$GOOGLE_EXTERNAL_ACCOUNT_TOKEN_TYPE" "$GOOGLE_EXTERNAL_ACCOUNT_INTERACTIVE" "$GOOGLE_EXTERNAL_ACCOUNT_OUTPUT_FILE" "$GOOGLE_EXTERNAL_ACCOUNT_IMPERSONATED_EMAIL" "$PIPKIN_HELPER_CUSTOM" > "$2"
if [ "$3" = timeout ]; then /bin/sleep 8; fi
if [ "$3" = stderr ]; then /bin/cat "$1" >&2; else /bin/cat "$1"; fi
if [ "$3" = fail ]; then exit 9; fi
`;
  await writeFile(join(temp, 'helper.sh'), script);
  const cases = [];
  for (const name of ['oidc', 'jwt', 'saml', 'cached', 'expired-cache', 'failure-cache', 'malformed-cache', 'missing-expiry', 'future-version', 'stderr', 'fail', 'denied', 'timeout']) {
    const type = `urn:ietf:params:oauth:token-type:${name === 'saml' ? 'saml2' : name === 'jwt' ? 'jwt' : 'id_token'}`;
    const response = {version:1, success:true, token_type:type, [name === 'saml' ? 'saml_response' : 'id_token']:'helper-token', expiration_time:4070908800};
    if (name === 'future-version') response.version = 2;
    if (name === 'missing-expiry') delete response.expiration_time;
    const doc = {type:'external_account', audience:'//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/provider', subject_token_type:type, token_url:'https://sts.example/v1/token', service_account_impersonation_url:'https://iam.example/v1/projects/-/serviceAccounts/helper@example:generateAccessToken', credential_source:{executable:{command:`/bin/sh "${temp}/helper.sh" "${temp}/reply" "${temp}/env" ${name}`, timeout_millis:5000}}};
    let cache = null;
    if (name.includes('cache') || name === 'cached' || name === 'missing-expiry') {
      doc.credential_source.executable.output_file = join(temp, 'output');
      cache = JSON.stringify({...response, id_token:'cached-token'});
      if (name === 'expired-cache') cache = JSON.stringify({...response, expiration_time:1});
      if (name === 'failure-cache') cache = JSON.stringify({version:1, success:false, code:'denied', message:'synthetic failure'});
      if (name === 'malformed-cache') cache = '{bad';
      await writeFile(join(temp, 'output'), cache);
    }
    await writeFile(join(temp, 'reply'), JSON.stringify(response));
    await rm(join(temp, 'env'), {force:true});
    process.env.GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES = name === 'denied' ? '0' : '1';
    process.env.PIPKIN_HELPER_CUSTOM = 'custom-value';
    const client = new GoogleAuth({scopes:['https://www.googleapis.com/auth/cloud-platform']}).fromJSON(doc);
    let outcome;
    try { outcome = {token:await client.retrieveSubjectToken()}; }
    catch (error) { outcome = {error:true, errorType:error.constructor.name}; }
    let env = null;
    try { env = (await readFile(join(temp, 'env'), 'utf8')).trimEnd().split('\n').map(v => v.replaceAll(temp, '$TEMP')); } catch {}
    cases.push({name, document:JSON.parse(JSON.stringify(doc).replaceAll(temp, '$TEMP')), response, cache, outcome, env});
  }
  const commands = ['/helper "two words" plain', '/helper \'two words\' $HOME | cat', '"/path with spaces/helper" "" x"y z"', '/helper "unterminated'];
  const parsing = commands.map(command => ({command, args:PluggableAuthHandler.parseCommand(command)}));
  await mkdir(dirname(output), {recursive:true});
  await writeFile(output, JSON.stringify({reference:pin, version:pkg.version, integrity:locked.integrity, hashes, scope:'Executed local SDK helpers and output-cache reads; synthetic credentials only; no cloud service', script, cases, parsing}, null, 2) + '\n');
} finally {
  if (previous === undefined) delete process.env.GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES;
  else process.env.GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES = previous;
  delete process.env.PIPKIN_HELPER_CUSTOM;
  await rm(temp, {recursive:true, force:true});
}
