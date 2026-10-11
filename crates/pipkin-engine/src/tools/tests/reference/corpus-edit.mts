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

function random(seed: number): () => number {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}
const PIECES = ["a", "b", "foo", " ", "  ", "\t", "\n", "\n", "\r\n", "‘", "’", "“", "”", "'", '"', "-", "—", "−", " ", "　", "ﬁ", "①", "Ａ", "é", "é", "😀", "中", "x", "y", "﻿"];
const SWAPS: Record<string, string[]> = { "'": ["’", "'"], '"': ["“", '"'], "-": ["—", "-"], " ": [" ", " "], "’": ["'"], "“": ['"'], "—": ["-"], " ": [" "], "ﬁ": ["fi"], "Ａ": ["A"] };
const pick = <T,>(next: () => number, xs: T[]): T => xs[Math.floor(next() * xs.length)]!;
const dir = mkdtempSync(join(tmpdir(), "corpus-edit-"));
const env = new NodeExecutionEnv({ cwd: dir });
const tool = createEditTool();
const api = { env, output: () => {}, diagnostic: () => {}, details: async () => {} } as any;
const cases: unknown[] = [];
for (let seed = 1; seed <= 1500; seed++) {
	const next = random(seed * 104729);
	let file = "";
	const n = 5 + Math.floor(next() * 60);
	for (let i = 0; i < n; i++) file += pick(next, PIECES);
	if (next() < 0.2) file = "﻿" + file;
	const editCount = 1 + Math.floor(next() * 3);
	const edits: { oldText: string; newText: string }[] = [];
	const chars = Array.from(file);
	for (let e = 0; e < editCount; e++) {
		const start = Math.floor(next() * chars.length);
		const len = 1 + Math.floor(next() * 8);
		let old = chars.slice(start, start + len).join("");
		if (next() < 0.5) old = Array.from(old).map((c) => (SWAPS[c] && next() < 0.7 ? pick(next, SWAPS[c]!) : c)).join("");
		if (next() < 0.2) old = old + "  ";
		const newText = Array.from({ length: Math.floor(next() * 4) }, () => pick(next, PIECES)).join("");
		edits.push({ oldText: old, newText });
	}
	writeFileSync(join(dir, "f.txt"), file);
	const args = { path: "f.txt", edits };
	let result: unknown;
	try {
		const r = await tool.execute(args, api, {});
		result = { ok: { text: r.content[0].text, details: r.details } };
	} catch (e) {
		result = { err: (e as Error).message };
	}
	cases.push({ id: seed, file, args, result, after: readFileSync(join(dir, "f.txt"), "utf8") });
}
console.log(JSON.stringify(cases));
