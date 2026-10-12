// Execute the pinned Models service's cycleThinking method with the original level selector.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-thinking-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const servicePath = 'packages/coding-agent/src/experimental/services/models-provider.ts';
const modelPath = 'packages/ai/src/models.ts';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong reference pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',servicePath,modelPath]);
const service = await readFile(join(root,servicePath),'utf8');
const models = await readFile(join(root,modelPath),'utf8');
const start = models.indexOf('const EXTENDED_THINKING_LEVELS:');
const end = models.indexOf('export function clampThinkingLevel',start);
if (start<0 || end<0) throw new Error('Cannot locate complete thinking-level function');
const temp = await mkdtemp(join(tmpdir(),'pipkin-thinking-reference-'));
try {
  await writeFile(join(temp,'service.ts'),service.replace(/from "[^"]+";/g,'from "./stubs.ts";'));
  await writeFile(join(temp,'levels.ts'),models.slice(start,end));
  await writeFile(join(temp,'stubs.ts'),`
    export {getSupportedThinkingLevels} from './levels.ts';
    export const clampThinkingLevel = () => { throw new Error('not used by cycle'); };
    export const defineFacet = value => value;
    export const BACKGROUND_CONTEXT = {};
    export const AgentDoc = {}, Models = {};
  `);
  const {createModelsService} = await import(pathToFileURL(join(temp,'service.ts')));
  const allNull = Object.fromEntries(['off','minimal','low','medium','high','xhigh','max'].map(x=>[x,null]));
  const cases = [
    {name:'missing-current',model:{reasoning:true}},
    {name:'ordinary',model:{reasoning:true},current:'medium'},
    {name:'wrap',model:{reasoning:true},current:'high'},
    {name:'unknown-current',model:{reasoning:true},current:'future'},
    {name:'disabled-current',model:{reasoning:true,thinkingLevelMap:{minimal:null}},current:'minimal'},
    {name:'mapped-extended',model:{reasoning:true,thinkingLevelMap:{xhigh:'extra',max:'maximum'}},current:'xhigh'},
    {name:'non-reasoning',model:{reasoning:false},current:'high'},
    {name:'missing-model',model:null,current:'high'},
    {name:'empty-levels',model:{reasoning:true,thinkingLevelMap:allNull},current:'medium'},
  ];
  for (const c of cases) {
    const agent = {value:{model:{provider:'fixture',modelId:'model'},...(c.current===undefined?{}:{thinkingLevel:c.current})}};
    let committed;
    const conversation = {configure:async change=>{committed=change.thinkingLevel;Object.assign(agent.value,change);}};
    const runtime = {getModel:()=>c.model??undefined};
    const createState = value=>({value,change:(_,f)=>f(value)});
    const {service} = createModelsService(conversation,agent,runtime,undefined,createState);
    const levels = await service.getThinkingLevels();
    await service.cycleThinking({});
    c.expected = {levels,thinking:committed};
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,sources:{[servicePath]:createHash('sha256').update(service).digest('hex'),[modelPath]:createHash('sha256').update(models).digest('hex')},scope:'Original createModelsService/cycleThinking and getSupportedThinkingLevels; stubbed document, configuration commit and facet dependencies; no network',cases},null,2)+'\n');
} finally { await rm(temp,{recursive:true,force:true}); }
