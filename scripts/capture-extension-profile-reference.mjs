// Capture only the pinned Pi profile helpers. No engine, provider, plugin bundle or user
// profile is started. All writes use a fresh temporary directory. Requires Pi's dependencies.
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { resolve, join, basename } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const [rootArg, outputArg] = process.argv.slice(2);
assert(rootArg && outputArg, 'Usage: node scripts/capture-extension-profile-reference.mjs PI_SOURCE OUTPUT_JSON');
const root = resolve(rootArg);
const file = join(root, 'packages/coding-agent/src/experimental/plugins/package.ts');
const sha = createHash('sha256').update(await readFile(file)).digest('hex');
assert.equal(sha, 'ea626c8a56aa2bf81b2b67646e60a9f1f0467633e4f8cf9836ba948dd2b0702f', 'reference source drift');
await import(pathToFileURL(join(root, 'packages/coding-agent/src/experimental/source-resolver.ts')).href);
const pi = await import(pathToFileURL(file).href);
const temp = await mkdtemp(join(tmpdir(), 'pipkin-profile-reference-'));
const observations = {};
const labels = paths => paths.map(path => basename(path));
try {
  const a = join(temp, 'questions');
  const b = join(temp, 'greeting');
  const one = join(temp, 'session-one');
  const empty = join(temp, 'session-empty');
  const server = 'profile-fixture';
  observations.server_absent = labels(await pi.restoreServerPluginPackageProfile(temp, server));
  observations.server_explicit = labels(await pi.restoreServerPluginPackageProfile(temp, server, [a,b]));
  observations.server_restored = labels(await pi.restoreServerPluginPackageProfile(temp, server));
  await pi.writeSessionPluginPackageProfile(temp, server, one, [a]);
  await pi.writeSessionPluginPackageProfile(temp, server, empty, []);
  observations.session_pinned = labels(await pi.readSessionPluginPackageProfile(temp, server, one));
  observations.session_empty = labels(await pi.readSessionPluginPackageProfile(temp, server, empty));
  observations.server_changed = labels(await pi.restoreServerPluginPackageProfile(temp, server, [b]));
  observations.session_after_default_change = labels(await pi.readSessionPluginPackageProfile(temp, server, one));
  observations.empty_after_default_change = labels(await pi.readSessionPluginPackageProfile(temp, server, empty));
  observations.server_cleared = labels(await pi.restoreServerPluginPackageProfile(temp, server, []));
  observations.server_after_clear = labels(await pi.restoreServerPluginPackageProfile(temp, server));
  observations.session_after_server_clear = labels(await pi.readSessionPluginPackageProfile(temp, server, one));
  try { pi.normalizePluginPackagePaths([a,a]); observations.duplicate_rejected = false; }
  catch { observations.duplicate_rejected = true; }
  await writeFile(join(temp, `plugin-packages-${server}.json`), '{"version":2,"packagePaths":[]}');
  try { await pi.restoreServerPluginPackageProfile(temp, server); observations.future_profile_rejected = false; }
  catch { observations.future_profile_rejected = true; }
  await writeFile(resolve(outputArg), JSON.stringify({
    referenceRevision: 'd2a311097cbcf669e699479587332ae3988a49d0',
    sourceSha256: sha,
    nodeVersion: process.version,
    scope: 'Original Pi profile helper calls; package paths mapped to native builtin labels. Does not execute facet packages or server/worker conflict and reload workflows.',
    observations,
  }, null, 2) + '\n');
} finally {
  await rm(temp, { recursive: true, force: true });
}
