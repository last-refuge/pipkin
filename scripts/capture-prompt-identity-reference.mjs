// Execute the pinned prompt builder with original tool contributions. External filesystem
// paths are fixed fixtures; skills are empty so their formatter is deliberately not exercised.
import {readFile, writeFile, mkdtemp, mkdir, rm} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {join, dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw new Error('Usage: node scripts/capture-prompt-identity-reference.mjs PI_SOURCE OUTPUT_JSON');
const reference = 'd2a311097cbcf669e699479587332ae3988a49d0';
const path = 'packages/coding-agent/src/core/system-prompt.ts';
const tools = ['read','write','edit','bash'];
const paths = [path,...tools.map(t=>`packages/coding-agent/src/core/tools/${t}.ts`)];
if (execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim() !== reference) throw new Error('Wrong reference pin');
execFileSync('git',['-C',root,'diff','--exit-code','HEAD','--',...paths]);
const sources = {};
const temp = await mkdtemp(join(tmpdir(),'pipkin-prompt-identity-'));
try {
  const source = await readFile(join(root,path),'utf8');
  sources[path] = createHash('sha256').update(source).digest('hex');
  await writeFile(join(temp,'prompt.ts'),source.replace(/from "[^"]+";/g,'from "./stubs.ts";'));
  await writeFile(join(temp,'stubs.ts'),`
    export const getSystemMessageText = () => { throw new Error('not used'); };
    export const getReadmePath = () => '/reference/README.md';
    export const getDocsPath = () => '/reference/docs';
    export const getExamplesPath = () => '/reference/examples';
    export const formatSkillsForPrompt = skills => { if(skills.length) throw new Error('unexpected skills'); return ''; };
  `);
  const contributions = {};
  for (const tool of tools) {
    const p = `packages/coding-agent/src/core/tools/${tool}.ts`;
    const source = await readFile(join(root,p),'utf8');
    sources[p] = createHash('sha256').update(source).digest('hex');
    const match = source.match(new RegExp(`export const ${tool}ToolSystemPromptContribution = \\{[\\s\\S]*?\\n\\} as const;`));
    if (!match) throw new Error(`Missing contribution ${tool}`);
    await writeFile(join(temp,`${tool}.ts`),match[0]);
    contributions[tool] = (await import(pathToFileURL(join(temp,`${tool}.ts`))))[`${tool}ToolSystemPromptContribution`];
  }
  const {buildSystemPromptSections} = await import(pathToFileURL(join(temp,'prompt.ts')));
  const cases = [[],tools,['read'],['powershell'],['bash','powershell'],['bash','grep'],['write','subagent'],['bash','read','read']].map((selectedTools,n)=>({
    name:`tools-${n}`,cwd:n===3?'C:\\repo':'/repo',selectedTools,
    contextFiles:n===1?[{path:'/repo/AGENTS.md',content:'Project instructions.'}]:[],
  }));
  for (const c of cases) {
    c.expected = buildSystemPromptSections({...c,skills:[],
      toolSnippets:Object.fromEntries(c.selectedTools.filter(t=>contributions[t]).map(t=>[t,contributions[t].snippet])),
      toolGuidelines:Object.fromEntries(c.selectedTools.filter(t=>contributions[t]).map(t=>[t,contributions[t].guidelines])),
    });
  }
  await mkdir(dirname(output),{recursive:true});
  await writeFile(output,JSON.stringify({reference,sources,scope:'Original prompt section builder and tool contribution constants. Fixed documentation paths, empty skills; no reference runtime or provider execution. Native intentionally replaces only preamble product name and docs.',cases},null,2)+'\n');
} finally { await rm(temp,{recursive:true,force:true}); }
