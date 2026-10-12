// Capture the pinned pure Pi prompt planner without installing its runtime dependencies.
// Only module import paths are rewritten in temporary copies; function bodies are untouched.
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';
const [reference, output] = process.argv.slice(2);
if (!reference || !output) throw new Error('Usage: node scripts/capture-system-reference.mjs PI_CHECKOUT OUTPUT_JSON');
const pin = 'd2a311097cbcf669e699479587332ae3988a49d0';
if (execFileSync('git', ['-C', reference, 'rev-parse', 'HEAD'], {encoding:'utf8'}).trim() !== pin) throw new Error('Unexpected reference revision');
const files = {
  'prompt.ts': 'packages/durable/src/harness/prompt.ts',
  'entries.ts': 'packages/durable/src/entries.ts',
  'transcript.ts': 'packages/ai/src/utils/transcript.ts',
  'text.ts': 'packages/ai/src/utils/text.ts',
};
const temp = await mkdtemp(join(tmpdir(), 'pipkin-system-reference-'));
try {
  execFileSync('git', ['-C', reference, 'diff', '--exit-code', 'HEAD', '--', ...Object.values(files)]);
  const sources = {};
  for (const [local, source] of Object.entries(files)) {
    const original = await readFile(join(reference, source), 'utf8');
    sources[source] = createHash('sha256').update(original).digest('hex');
    const rewritten = original.replace('"@earendil-works/pi-ai/utils/transcript"', '"./transcript.ts"').replace('"../entries.ts"', '"./entries.ts"');
    await writeFile(join(temp, local), rewritten);
  }
  const {planSystemEntries} = await import(pathToFileURL(join(temp, 'prompt.ts')));
  const {collapseSystemMessages} = await import(pathToFileURL(join(temp, 'transcript.ts')));
  const tool = (name, description=name) => ({name, description, parameters:{type:'object'}});
  const system = (id, sections, toolsAdded=[], toolsRemoved=[]) => ({id, kind:'pi.system', model:[{role:'system', content:'', sections, ...(toolsAdded.length ? {toolsAdded} : {}), ...(toolsRemoved.length ? {toolsRemoved: toolsRemoved.map(name=>({name}))} : {}), timestamp:id}]});
  const first = system(1, {z:'Z', a:'A'}, [tool('read'),tool('write')]);
  const cases = [
    {name:'initial', entries:[], desired:{z:'Z',a:'A'}, tools:[tool('read')]},
    {name:'unchanged', entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('read'),tool('write')]},
    {name:'minimal', entries:[first], desired:{z:'Z2',b:'B'}, tools:[tool('read'),tool('write')]},
    {name:'reorder', entries:[first], desired:{a:'A',z:'Z'}, tools:[tool('read'),tool('write')]},
    {name:'tool-addition', entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('read'),tool('write'),tool('bash')]},
    {name:'redefinition-order', entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('read','changed'),tool('write')]},
    {name:'tool-removal', entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('write')]},
    {name:'tool-reorder', entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('write'),tool('read')]},
    {name:'compaction-baseline', head:10, entries:[first], desired:{z:'Z',a:'A'}, tools:[tool('read'),tool('write')]},
    {name:'compaction-already-rebased', head:10, entries:[system(11,{z:'Z',a:'A'},[tool('read')])], desired:{z:'Z',a:'A'}, tools:[tool('read')]},
  ];
  for (const c of cases) {
    const view = {head: c.head === undefined ? undefined : {id:c.head,kind:'pi.compaction',head:1}, entries:c.entries, messages:c.entries.flatMap(e=>e.model), contributions:c.entries.map(e=>e.model)};
    c.planned = planSystemEntries(view, new Map(Object.entries(c.desired)), c.tools, 100);
    const omitted = new Set(c.planned.flatMap(e=>(e.edits??[]).map(edit=>edit.target)));
    c.collapsed = collapseSystemMessages({messages:[...c.entries.filter(e=>!omitted.has(e.id)).flatMap(e=>e.model),...c.planned.flatMap(e=>e.model)]}).messages;
  }
  await writeFile(output, JSON.stringify({reference:pin, sources, cases}, null, 2)+'\n');
} finally { await rm(temp, {recursive:true, force:true}); }
