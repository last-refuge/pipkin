// Execute pinned Pi's constrained-sampling helpers and Responses tool conversion.
// Pure function bodies are unmodified; type-only imports are erased by Node's TS loader.
import {readFile, writeFile, mkdtemp, rm, mkdir} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-constrained-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected reference revision');
const files = ['packages/ai/src/api/constrained-sampling.ts','packages/ai/src/api/openai-responses-shared.ts'];
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--',...files]);
const temp = await mkdtemp(join(tmpdir(),'pipkin-constrained-reference-'));
try {
  const sources = {}, originals = [];
  for (const path of files) {
    const text = await readFile(join(reference,path),'utf8');
    originals.push(text);
    sources[path] = createHash('sha256').update(text).digest('hex');
  }
  await writeFile(join(temp,'sampling.ts'),originals[0]);
  const start = originals[1].indexOf('export function convertResponsesTools(');
  const end = originals[1].indexOf('\n// =============================================================================',start);
  if (start < 0 || end < 0) throw new Error('Tool converter not found');
  await writeFile(join(temp,'tools.ts'), 'import {resolveGrammarConstrainedSampling, resolveJsonSchemaStrictSampling, getJsonSchemaToolParameters} from "./sampling.ts";\n'+originals[1].slice(start,end));
  const {convertResponsesTools} = await import(pathToFileURL(join(temp,'tools.ts')));
  const {resolveGrammarConstrainedSampling} = await import(pathToFileURL(join(temp,'sampling.ts')));
  const tool = {name:'patch',description:'Apply a patch',parameters:{type:'object',properties:{patch:{type:'string'}},required:['patch']}};
  const gram = {type:'grammar',variants:{openai_lark:'start: /.+/s',openai_regex:'.+'}};
  const cases = [];
  const add = (name,config,options={},parameters=tool.parameters) => {
    const t = {...tool,parameters,...(config?{constrainedSampling:config}:{})};
    const c = {name,tool:t,options};
    try { c.tools=convertResponsesTools([t],options); c.grammar=resolveGrammarConstrainedSampling(t,options.supportsOpenAIGrammarTools??false)??null; }
    catch(e) { c.error=e.message; }
    cases.push(c);
  };
  add('ordinary',null);
  add('codex-null-strict',null,{strict:null});
  add('strict-required',{type:'json_schema',strict:'require'}, {}, {type:'object',properties:{patch:{type:'string'},note:{type:'string'}},required:['patch']});
  add('strict-unsupported-required',{type:'json_schema',strict:'require'},{supportsStrictMode:false});
  add('strict-unsupported-prefer',{type:'json_schema',strict:'prefer'},{supportsStrictMode:false});
  add('schema-unsupported-required',{type:'json_schema',strict:'require'},{},{type:'object',oneOf:[]});
  add('schema-unsupported-prefer',{type:'json_schema',strict:'prefer'},{},{type:'object',oneOf:[]});
  add('grammar-lark-preferred',gram,{supportsOpenAIGrammarTools:true});
  add('grammar-regex',{type:'grammar',variants:{openai_lark:' ',openai_regex:'.+'}},{supportsOpenAIGrammarTools:true});
  add('grammar-fallback',gram,{supportsOpenAIGrammarTools:false});
  add('grammar-missing-variant',{type:'grammar',variants:{}},{supportsOpenAIGrammarTools:true});
  add('grammar-non-object',gram,{supportsOpenAIGrammarTools:true},{type:'string'});
  add('grammar-many-required',gram,{supportsOpenAIGrammarTools:true},{...tool.parameters,required:['patch','other']});
  add('grammar-missing-property',gram,{supportsOpenAIGrammarTools:true},{type:'object',required:['patch']});
  add('grammar-non-string',gram,{supportsOpenAIGrammarTools:true},{type:'object',properties:{patch:{type:'number'}},required:['patch']});
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference:pin,sources,cases},null,2)+'\n');
} finally { await rm(temp,{recursive:true,force:true}); }
