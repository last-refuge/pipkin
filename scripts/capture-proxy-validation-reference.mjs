// Execute the pinned reference proxy resolver. No network traffic or real credentials.
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { join, dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-proxy-validation-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
const files = ['packages/ai/src/utils/node-http-proxy.ts', 'packages/ai/src/utils/provider-env.ts'];
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected revision');
execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', ...files]);
for (const key of Object.keys(process.env)) if (/^(https?|all|no)_proxy$/i.test(key)) delete process.env[key];
const {resolveHttpProxyUrlForTarget} = await import(pathToFileURL(resolve(reference, files[0])).href);
const cases = [];
for (const [name, target, env] of [
  ['socks', 'http://provider.test/', {HTTP_PROXY:'socks5://127.0.0.1:1080'}],
  ['pac', 'https://provider.test/', {HTTPS_PROXY:'pac+http://wpad/proxy.pac'}],
  ['malformed', 'http://provider.test/', {HTTP_PROXY:'http://[bad'}],
  ['excluded-invalid', 'http://provider.test/', {HTTP_PROXY:'socks5://127.0.0.1:1080', NO_PROXY:'provider.test'}],
  ['excluded-port', 'http://provider.test:80/', {HTTP_PROXY:'socks5://127.0.0.1:1080', NO_PROXY:'provider.test:80'}],
  ['wrong-port', 'http://provider.test:81/', {HTTP_PROXY:'socks5://127.0.0.1:1080', NO_PROXY:'provider.test:80'}],
  ['other-scheme', 'https://provider.test/', {HTTP_PROXY:'socks5://127.0.0.1:1080'}],
  ['valid', 'https://provider.test/', {HTTPS_PROXY:'http://localhost:8080'}],
]) {
  let result;
  try { result = {proxy:resolveHttpProxyUrlForTarget(target, env)?.href ?? null}; }
  catch (error) { result = {error:error.message}; }
  cases.push({name, target, env, ...result});
}
const hashes = {};
for (const file of files) hashes[file] = createHash('sha256').update(await readFile(join(reference,file))).digest('hex');
await mkdir(dirname(output),{recursive:true});
await writeFile(output, JSON.stringify({reference:pin,node:process.version,sourceHashes:hashes,scope:'Executed pinned resolver directly; no network requests or complete application execution',cases},null,2)+'\n');
