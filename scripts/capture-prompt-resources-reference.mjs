// Execute the original worker-local resource cache with controlled resource loaders.
// Only import paths are rewritten; no production function body is replaced.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-prompt-resources-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const path = 'packages/coding-agent/src/experimental/durable/prompt.ts';
if (execFileSync('git', ['-C',root,'rev-parse','HEAD'], {encoding:'utf8'}).trim() !== reference) throw new Error('Wrong reference pin');
execFileSync('git', ['-C',root,'diff','--exit-code','HEAD','--',path]);
const source = await readFile(join(root,path),'utf8');
const temp = await mkdtemp(join(tmpdir(),'pipkin-prompt-resources-reference-'));
try {
  await writeFile(join(temp,'prompt.ts'),source.replace(/from "[^"]+";/g,'from "./stubs.ts";'));
  await writeFile(join(temp,'stubs.ts'), `
    let version = 1;
    export const setVersion = v => { version = v; };
    export const defineExtension = value => value;
    export const section = (key, render) => ({key, render});
    export const getAgentDir = () => '/fixture-config';
    export const loadProjectContextFiles = ({cwd}) => [{path: cwd+'/AGENTS.md', content: 'instruction-'+version}];
    export const loadSkills = () => ({skills:[{description:'skill-'+version}]});
    export const buildSystemPromptSections = input => ({
      project_context:input.contextFiles[0].content,
      skills:input.skills[0].description,
      tools:input.selectedTools,
    });
    const contribution = {snippet:'fixture',guidelines:[]};
    export const bashToolSystemPromptContribution = contribution;
    export const editToolSystemPromptContribution = contribution;
    export const readToolSystemPromptContribution = contribution;
    export const writeToolSystemPromptContribution = contribution;
  `);
  const {createPiPrompt} = await import(pathToFileURL(join(temp,'prompt.ts')));
  const {setVersion} = await import(pathToFileURL(join(temp,'stubs.ts')));
  const cases = [
    {group:'root',cwd:'a',version:1,tools:['read','write']},
    {group:'root',cwd:'a',version:2,tools:['bash']},
    {group:'root',cwd:'a',version:2,tools:['read','write']},
    {group:'root',cwd:'b',version:2,tools:['read']},
    {group:'independent',cwd:'a',version:2,tools:['read']},
    {group:'independent',cwd:'a',version:3,tools:['read','bash']},
    {group:'restarted',cwd:'a',version:3,tools:['read']},
  ];
  const groups = new Map();
  for (const c of cases) {
    setVersion(c.version);
    if (!groups.has(c.group)) groups.set(c.group,createPiPrompt({getSkillPaths:()=>[]},c.cwd));
    const input = {agent:{cwd:c.cwd,tools:c.tools.map(name=>({name}))}};
    c.expected = Object.fromEntries(groups.get(c.group).sections.map(s=>[s.key,s.render(input)]).filter(([,v])=>v!==undefined));
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,sources:{[path]:createHash('sha256').update(source).digest('hex')},scope:'Original createPiPrompt resource-cache function; controlled resource loaders and section renderer, no provider/network/real filesystem reference loads',cases},null,2)+'\n');
} finally {
  await rm(temp,{recursive:true,force:true});
}
