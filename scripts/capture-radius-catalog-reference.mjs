// Execute the pinned Radius config sanitizer/converter; no provider or account requests.
import {readFile, writeFile, mkdir} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-radius-catalog-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const path = 'packages/ai/src/providers/radius-config.ts';
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong reference pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',path]);
const source = await readFile(join(root,path),'utf8');
const {getRadiusModels} = await import(pathToFileURL(join(root,path)));
const model = {id:'private-model',name:'Gateway model',reasoning:true,input:['text','image'],cost:{input:1,output:2,cacheRead:0.25,cacheWrite:0.5},contextWindow:32000,maxTokens:4096};
const cases = [
  {name:'valid',provider:'radius',config:{baseUrl:'https://gateway.example/v1',models:[model]}},
  {name:'custom-and-extra',provider:'team',config:{baseUrl:'https://team.example/v1',models:[{...model,provider:'wrong',api:'wrong',baseUrl:'wrong',thinkingLevelMap:{low:"gateway-low",high:null},extraMetadata:'kept'}]}},
  {name:'filter-invalid',provider:'team',config:{baseUrl:'https://team.example/v1',models:[null,[],{id:'incomplete'},{...model,cost:[]},model]}},
  {name:'empty',provider:'radius',config:{baseUrl:'https://gateway.example/v1',models:[]}},
  {name:'bad-envelope',provider:'radius',config:{baseUrl:1,models:[model]}},
  {name:'bad-list',provider:'radius',config:{baseUrl:'https://gateway.example/v1',models:{}}},
];
for (const c of cases) c.expected = getRadiusModels(c.provider,{type:'oauth',access:'invented',refresh:'invented',expires:1,gatewayConfig:c.config});
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference,sources:{[path]:createHash('sha256').update(source).digest('hex')},scope:'Executed original Radius config sanitizer/converter only; no network or provider lifecycle qualification.',cases},null,2)+'\n');
