// Execute the pinned model resolver with its real locked minimatch dependency.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, installed, output, orderingOnly] = process.argv.slice(2);
if (!output) throw new Error('Usage: node scripts/capture-model-scope-reference.mjs PI_SOURCE PI_INSTALLED OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const sourcePath = 'packages/coding-agent/src/core/model-resolver.ts';
const argsPath = 'packages/coding-agent/src/cli/args.ts';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',sourcePath,argsPath,'package-lock.json']);
const source = await readFile(join(root,sourcePath),'utf8');
const args = await readFile(join(root,argsPath),'utf8');
const lock = JSON.parse(await readFile(join(root,'package-lock.json'),'utf8'));
const require = createRequire(join(installed,'package.json'));
const minimatchPath = require.resolve('minimatch');
const minimatchPackage = JSON.parse(await readFile(require.resolve('minimatch/package.json'),'utf8'));
if (minimatchPackage.version !== lock.packages['node_modules/minimatch'].version) throw new Error('Wrong minimatch version');
const bracePath = createRequire(minimatchPath).resolve('brace-expansion');
const bracePackage = JSON.parse(await readFile(join(dirname(bracePath),'../../package.json'),'utf8'));
if (bracePackage.version !== lock.packages['node_modules/brace-expansion'].version) throw new Error('Wrong brace-expansion version');
const dependencySha256 = {};
for (const [name, path] of [
  ['minimatch/index.js', minimatchPath],
  ['minimatch/ast.js', join(dirname(minimatchPath),'ast.js')],
  ['minimatch/brace-expressions.js', join(dirname(minimatchPath),'brace-expressions.js')],
  ['brace-expansion/index.js', bracePath],
]) dependencySha256[name] = createHash('sha256').update(await readFile(path)).digest('hex');
const levels = args.slice(args.indexOf('const VALID_THINKING_LEVELS ='),args.indexOf('export function normalizeSessionName'));
if (!levels.includes('export function isValidThinkingLevel')) throw new Error('Missing level validation');
const temp = await mkdtemp(join(tmpdir(),'pipkin-model-scope-'));
try {
  await writeFile(join(temp,'resolver.ts'),source
    .replace('from "@earendil-works/pi-ai";', 'from "./stubs.mjs";')
    .replace('import chalk from "chalk";', 'import {chalk} from "./stubs.mjs";')
    .replace('from "minimatch";', `from ${JSON.stringify(pathToFileURL(minimatchPath).href)};`)
    .replace('from "../cli/args.ts";', 'from "./levels.ts";')
    .replace('from "./defaults.ts";', 'from "./stubs.mjs";'));
  await writeFile(join(temp,'levels.ts'),levels);
  await writeFile(join(temp,'stubs.mjs'),`
    export const modelsAreEqual = (a,b) => a.id===b.id && a.provider===b.provider;
    export const DEFAULT_THINKING_LEVEL = 'medium';
    export const chalk = new Proxy({}, {get(){throw new Error('Unused formatting dependency called');}});
  `);
  const {resolveModelScopeFromModels: resolve} = await import(pathToFileURL(join(temp,'resolver.ts')));
  const model = (id,provider='p',name=id) => ({provider,id,name});
  const models = [
    ...['alpha','alpha-latest','alpha-20250101','alpha-20260101','beta','beta-large','gamma',
      '.hidden','.','..','a/b','a/.hidden','a/b/c','x1','x2','x3','x01','x02','x03','xa','xb','xc',
      'α','é','😀','A','a','aZ','az','a_z','a-z','a.z','a z','a+z','a:z','star*','what?',
      'bracket[','choice{a,b}','back\\slash','line\nbreak','a/../c','a/c','model:exacto'].map(id=>model(id)),
    model('alpha','other'), model('extra','other','Friendly Name'), model('aZ','q'),
  ];
  const patterns = [
    [], ['alpha'], ['p/alpha'], [' P / alpha '], ['ALPHA'], ['missing'], ['Friendly'],
    ['alpha:high'], ['model:exacto:low'], ['alpha:bad:high'], ['alpha:low:bad'],
    ['p/*:high','alpha:low'], ['*'], ['**'], ['p/*'], ['p/**'], ['a/*'], ['a/**'], ['**/c'],
    ['.*'], ['[.]hidden'], ['**/.hidden'], ['p//a/*'], ['p/a/../*'], ['p/./*'], ['a/*/'],
    ['#*'], ['!*'], ['!p/*'], ['!!p/*'], ['!alpha*'], ['!(alpha|beta)*'],
    ['{alpha,beta}*'], ['x{1..3}*'], ['x{03..01}*'], ['x{a..c}*'], ['x{1..3..2}*'],
    ['{a,{b,g}amma}*'], ['{alpha,beta}'], ['x{1..3}'],
    ['*(alpha|beta)'], ['+(alpha|beta)*'], ['?(alpha|beta)*'], ['@(alpha|beta)*'],
    ['p/!(alpha|beta)*'], ['p/!(*a)'], ['p/!()'], ['p/*(a|*(b|c))'], ['p/?()'],
    ['p/@(.*|*)'], ['p/+(*|.h*)'], ['p/*(x|a)*'], ['p/*(alpha|beta'],
    ['[a-c]*'], ['[!a-c]*'], ['[^a-c]*'], ['[[:alpha:]]*'], ['[[:digit:]]*'],
    ['bracket['], ['star\\*'], ['what\\?'], ['back\\\\*'], ['?'], ['??'], ['a*'],
    ['p/a?'], ['p/A*'], ['p/model*:high'], ['p/*:bad'],
  ];
  const cases=patterns.map((patterns,i)=>({name:`scope-${i}`,patterns}));
  const words=['', '.', '..', 'a', 'b', 'c', 'x', 'ab', 'abc', 'aaa', 'bbb', 'a.b', '.ab', 'ba', 'bc', 'abcd', 'xabc', 'abcx', 'a/b', 'axby', 'a\\b', 'a-1', '😀', 'é', 'K', 'ſ', 'ı', 'I', '{a}', '{b}', 'a}'];
  const edges=['p/*(a|b)!(c)*', 'p/!(a|b)c*', 'p/a!(b)c*', 'p/*!(a|)*',
    'p/@(a|!(b))*', 'p/!(@(a|b))*', 'p/*(!(a)|b)', 'p/*(a|+(b|c))',
    'p/*(?(a)|b)', 'p/+(*(a)|b)', 'p/*(a|!(b|c))', 'p/!(a|b)*c',
    'p/+(?)', 'p/*(?)', 'p/+(.*|*)', 'p/?()x*', 'p/+()x*', 'p/*()x*',
    'p/[z-a]*','p/[z-aq]*','p/[]a]*','p/[!]]*','p/[[:print:]]*','p/[[:graph:]]*',
    'p/[a-[:alpha:]]*','p/[[:upper:]]*','p/[[:word:]]*','p/[😀]*',
    'p/{a,}*','p/{,}*','p/{{a,b}}*','p/{a,{b,c}}*','p/{a},b}*',
    'p/{a,b}{c,x}*','p/{a..c..0}*','p/{a..c..-2}*','p/{-01..1}*',
    'p/a/**/','p/**/**','p/**/../*','p/a/*/../*', 'p/[k]*','p/[s]*','p/[i]*','p/😀*','p/[[:alpha:]😀]*','p/[[:lower:]]*'];
  for (const pattern of edges) cases.push({name:`edge-${cases.length}`,models:words.map(id=>model(id)),patterns:[pattern]});
  // Deterministic compositional corpus; expectation is always obtained by executing Pi.
  let seed=0x7069706b;
  const choose=a=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return a[seed%a.length];};
  const atom=()=>choose(['a','b','c','*','?','[a-b]','.','']);
  const expr=depth=>depth===0 || choose([true,false]) ? atom() : choose(['*','+','?','@','!'])+'('+expr(depth-1)+'|'+expr(depth-1)+')';
  for(let i=0;i<160;i++) cases.push({name:`composed-${i}`,models:words.map(id=>model(id)),patterns:['p/'+expr(3)+expr(2)+'*']});
  const pairs=[['z','ä'],['Z','a'],['a_z','a-z'],['a-z','a.z'],['a.z','a+z'],['a','A'],
    ['é','e\u0301'],['i','İ'],['ss','ß'],['😀','z'],['x9','x10'],['ı','I'],['İ','i']];
  for (const [a,b] of pairs) for(const order of [[a,b],[b,a]])
    cases.push({name:`fuzzy-${cases.length}`,models:order.map(id=>model(id,'p','choose')),patterns:['choose']});
  cases.push({name:'dated-only',models:['x-20250101','x-20260101'].map(id=>model(id)),patterns:['x']});
  cases.push({name:'alias-before-dated',models:['x-20290101','x-latest'].map(id=>model(id)),patterns:['x']});
  const selectedCases=orderingOnly==='--ordering-only' ? cases.filter(c=>c.name.startsWith('fuzzy-') || c.name==='dated-only' || c.name==='alias-before-dated') : cases;
  for (const c of selectedCases) {
    const r=resolve(c.patterns,c.models??models);
    c.expected={models:r.scopedModels.map(x=>({key:`${x.model.provider}/${x.model.id}`,thinking:x.thinkingLevel??null})),warnings:r.diagnostics.map(x=>x.message)};
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,node:process.version,locale:new Intl.Collator().resolvedOptions().locale,minimatch:minimatchPackage.version,
    braceExpansion:bracePackage.version,dependencySha256,
    sourceSha256:createHash('sha256').update(source).digest('hex'),argsSha256:createHash('sha256').update(args).digest('hex'),
    catalog:orderingOnly==='--ordering-only' ? [] : models,scope:'Actual resolveModelScopeFromModels and level validator; minimatch from matching locked version. Equality restricted to chat-model fixtures; unused chalk throws. Default Node locale recorded.',cases:selectedCases},null,2)+'\n');
} finally {await rm(temp,{recursive:true,force:true});}
