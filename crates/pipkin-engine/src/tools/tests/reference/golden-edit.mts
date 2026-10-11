// Generates expected outputs from the reference implementation (Pi durable engine).
// Usage: PI_REF=<checkout with node_modules installed> node <this file> > <output.json>
// golden-*.mts write the golden JSON files beside the tests; corpus-*.mts write corpora for
// the ignored tests (`cargo test -- --ignored` with PIPKIN_READ_CORPUS / PIPKIN_EDIT_CORPUS).
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const R = `${process.env.PI_REF ?? "."}/packages`;
const { createEditTool } = await import(`${R}/durable/src/tools/index.ts`);
const { NodeExecutionEnv } = await import(`${R}/durable/src/env/node.ts`);
const BACKGROUND_CONTEXT = {} as any;

type Case = { name: string; file: string; edits: unknown };
const cases: Case[] = [
	{ name: "disjoint", file: "alpha\nbeta\ngamma\ndelta\n", edits: [{ oldText: "alpha\n", newText: "ALPHA\n" }, { oldText: "gamma\n", newText: "GAMMA\n" }] },
	{ name: "crlf_bom", file: "﻿one\r\ntwo\r\n", edits: [{ oldText: "two", newText: "TWO" }] },
	{ name: "fuzzy_quotes", file: "const a = “hello”;   \nconst b = ‘x’;\nkeep this   \n", edits: [{ oldText: 'const a = "hello";', newText: 'const a = "bye";' }] },
	{ name: "fuzzy_dash_trailing", file: "line one  \nvalue — dash\t\nlast　line \n", edits: [{ oldText: "value - dash\nlast line", newText: "VALUE\nLAST" }] },
	{ name: "fuzzy_nfkc", file: "ＡＢＣ width\nnext\n", edits: [{ oldText: "ABC width", newText: "abc" }] },
	{ name: "mixed_exact_fuzzy", file: "a  \nb’s\nc\nd\n", edits: [{ oldText: "c\n", newText: "C\n" }, { oldText: "b's", newText: "B" }] },
	{ name: "overlap", file: "one\ntwo\nthree\n", edits: [{ oldText: "one\ntwo\n", newText: "x" }, { oldText: "two\nthree\n", newText: "y" }] },
	{ name: "dup_multi", file: "x\nx\ny\n", edits: [{ oldText: "y", newText: "z" }, { oldText: "x", newText: "w" }] },
	{ name: "missing_multi", file: "x\ny\n", edits: [{ oldText: "y", newText: "z" }, { oldText: "q", newText: "w" }] },
	{ name: "empty_old", file: "x\n", edits: [{ oldText: "", newText: "z" }] },
	{ name: "empty_old_multi", file: "x\n", edits: [{ oldText: "x", newText: "z" }, { oldText: "", newText: "z" }] },
	{ name: "no_change", file: "x\n", edits: [{ oldText: "x", newText: "x" }] },
	{ name: "no_change_multi", file: "x\ny\n", edits: [{ oldText: "x", newText: "x" }, { oldText: "y", newText: "y" }] },
	{ name: "no_trailing_newline", file: "a\nb\nc", edits: [{ oldText: "c", newText: "C\nD" }] },
	{ name: "delete_all", file: "a\nb\n", edits: [{ oldText: "a\nb\n", newText: "" }] },
	{ name: "insert_into_empty_lines", file: "\n\n\nfoo\n\n\n", edits: [{ oldText: "foo", newText: "bar\nbaz" }] },
	{ name: "long_context", file: Array.from({ length: 40 }, (_, i) => `l${i + 1}`).join("\n") + "\n", edits: [{ oldText: "l5\n", newText: "L5\n" }, { oldText: "l20\n", newText: "" }, { oldText: "l38", newText: "L38\nextra" }] },
	{ name: "close_changes", file: Array.from({ length: 20 }, (_, i) => `r${i + 1}`).join("\n") + "\n", edits: [{ oldText: "r5\n", newText: "R5\n" }, { oldText: "r12\n", newText: "R12\n" }] },
	{ name: "unicode", file: "héllo wörld 😀\nz\n", edits: [{ oldText: "wörld 😀", newText: "世界" }] },
	{ name: "lone_cr", file: "a\rb\rc\n", edits: [{ oldText: "b\n", newText: "B\n" }] },
	{ name: "crlf_input_old", file: "a\r\nb\r\n", edits: [{ oldText: "a\r\nb", newText: "x\r\ny" }] },
	{ name: "fuzzy_dup", file: "it’s\nit's\n", edits: [{ oldText: "it's", newText: "z" }] },
	{ name: "legacy_args", file: "aaa\n", edits: undefined },
];

const out: unknown[] = [];
for (const c of cases) {
	const dir = mkdtempSync(join(tmpdir(), "golden-edit-"));
	writeFileSync(join(dir, "f.txt"), c.file);
	const env = new NodeExecutionEnv({ cwd: dir });
	const tool = createEditTool();
	let args: any = c.edits === undefined ? { path: "f.txt", oldText: "aaa", newText: "bbb" } : { path: "f.txt", edits: c.edits };
	args = tool.prepareArguments(args);
	const api = { env, output: () => {}, diagnostic: () => {}, details: async () => {} } as any;
	let result: unknown;
	try {
		const r = await tool.execute(args, api, BACKGROUND_CONTEXT);
		result = { ok: r };
	} catch (e) {
		result = { err: (e as Error).message };
	}
	out.push({ name: c.name, file: c.file, args, result, after: readFileSync(join(dir, "f.txt"), "utf8") });
}
console.log(JSON.stringify(out, null, 1));
