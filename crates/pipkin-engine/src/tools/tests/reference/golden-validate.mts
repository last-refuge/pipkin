// Generates expected outputs from the reference implementation (Pi durable engine).
// Usage: PI_REF=<checkout with node_modules installed> node <this file> > <output.json>
// golden-*.mts write the golden JSON files beside the tests; corpus-*.mts write corpora for
// the ignored tests (`cargo test -- --ignored` with PIPKIN_READ_CORPUS / PIPKIN_EDIT_CORPUS).
const R = `${process.env.PI_REF ?? "."}/packages`;
const { validateToolArguments } = await import(`${R}/ai/src/utils/validation.ts`);
const { createReadTool, createEditTool, createBashTool, createWriteTool } = await import(`${R}/durable/src/tools/index.ts`);
const tools: Record<string, any> = { read: createReadTool(), edit: createEditTool(), bash: createBashTool(), write: createWriteTool() };
const cases: [string, unknown][] = [
	["read", {}],
	["read", { path: 5 }],
	["read", { path: "a", offset: "12" }],
	["read", { path: "a", offset: "x" }],
	["read", { path: "a", offset: null }],
	["read", { path: "a", limit: true }],
	["read", { path: null }],
	["read", { path: "a", extra: 1 }],
	["read", { path: ["a"] }],
	["read", { path: { a: 1 } }],
	["read", { path: "a", offset: " 3 " }],
	["read", { path: "a", offset: "" }],
	["read", { path: "a", offset: "1e2" }],
	["read", { path: "a", offset: "0x10" }],
	["read", { path: "a", offset: [] }],
	["read", { path: ["a"], offset: "x", limit: {} }],
	["read", { path: "a", offset: "Infinity" }],
	["read", { path: "a", offset: "TRUE" }],
	["read", { path: "a", offset: "-5.5" }],
	["read", { path: "a", offset: false }],
	["read", { path: "a", offset: "1_000" }],
	["read", { path: "a", offset: "+5" }],
	["read", { path: "a", offset: ".5" }],
	["read", { path: "a", offset: "5." }],
	["read", { path: "a", offset: "0b11" }],
	["read", { path: "a", offset: "0o17" }],
	["read", { path: "a", offset: "-0x10" }],
	["read", { path: "a", offset: "1e400" }],
	["read", { path: "a", offset: "1e" }],
	["read", { offset: "x" }],
	["read", "str"],
	["read", null],
	["read", [1]],
	["edit", { path: "a" }],
	["edit", { path: "a", edits: [{ oldText: "x" }] }],
	["edit", { path: "a", edits: "nope" }],
	["edit", { path: "a", edits: [{ oldText: 1, newText: 2 }] }],
	["edit", { path: "a", edits: [5] }],
	["edit", { edits: [] }],
	["edit", { path: "a", edits: { oldText: "a" } }],
	["edit", { path: "a", edits: [{ oldText: "a", newText: "b", x: 1 }] }],
	["edit", { path: "a", edits: '[{"oldText":"a","newText":"b"}]' }],
	["edit", { path: "a", oldText: "a", newText: "b" }],
	["edit", { path: "a", edits: [], oldText: "a", newText: "b" }],
	["edit", { path: "a", edits: '{"oldText":"a","newText":"b"}', oldText: "c", newText: "d" }],
	["edit", { path: "a", edits: null }],
	["edit", { path: "a", edits: [null] }],
	["edit", { edits: [{ newText: 1 }, { oldText: [], newText: {} }] }],
	["edit", { path: "a", edits: [{}] }],
	["bash", { command: "ls", timeout: "10" }],
	["bash", { command: true }],
	["bash", { command: 1.5 }],
	["bash", { command: "x", timeout: null }],
	["bash", { command: {} }],
	["write", { path: "a" }],
	["write", {}],
	["write", { path: true, content: 12.5 }],
	["write", { path: "a", content: 1e21 }],
	["write", { path: "a", content: null }],
	["write", { path: "a", content: [1] }],
];
const out: unknown[] = [];
for (const [name, args] of cases) {
	const tool = tools[name];
	const prepared = tool.prepareArguments ? tool.prepareArguments(args) : args;
	try {
		out.push({ tool: name, args, ok: validateToolArguments(tool, { type: "toolCall", id: "c", name, arguments: prepared }) });
	} catch (e) {
		out.push({ tool: name, args, err: (e as Error).message });
	}
}
console.log(JSON.stringify(out, null, 1));
