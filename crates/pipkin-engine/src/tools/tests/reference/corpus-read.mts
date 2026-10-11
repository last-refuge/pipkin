// Generates expected outputs from the reference implementation (Pi durable engine).
// Usage: PI_REF=<checkout with node_modules installed> node <this file> > <output.json>
// golden-*.mts write the golden JSON files beside the tests; corpus-*.mts write corpora for
// the ignored tests (`cargo test -- --ignored` with PIPKIN_READ_CORPUS / PIPKIN_EDIT_CORPUS).
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const R = `${process.env.PI_REF ?? "."}/packages`;
const { createReadTool } = await import(`${R}/durable/src/tools/index.ts`);
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
const PIECES = [[0x0a], [0x0a], [0x61], [0x62, 0x63, 0x64, 0x65], [0xef, 0xbb, 0xbf], [0xc3, 0xa9], [0xe2, 0x82, 0xac], [0xf0, 0x9f, 0x98, 0x80], [0xe2, 0x82], [0xff], [0x0d, 0x0a], [0xed, 0xa0, 0x80], [0x0b, 0x1b]];
function randomFile(next: () => number): Uint8Array {
	const kind = next();
	if (kind < 0.08) return Uint8Array.from([0xef, 0xbb, 0xbf, ...new TextEncoder().encode("x\n".repeat(2100))]);
	if (kind < 0.16) {
		const line = new Uint8Array(51200 + 200 + Math.floor(next() * 400)).fill(0x61);
		line[51200 - 1 + Math.floor(next() * 3)] = 0xc3;
		return Uint8Array.from([...line, 0x0a, 0x62]);
	}
	if (kind < 0.22) return new TextEncoder().encode(`${"0123456789".repeat(30)}\n`.repeat(200));
	if (kind < 0.28) return new TextEncoder().encode(`${"é".repeat(150)}\r\n`.repeat(250));
	const bytes: number[] = [];
	const pieces = Math.floor(next() * 120);
	for (let i = 0; i < pieces; i++) bytes.push(...PIECES[Math.floor(next() * PIECES.length)]!);
	return Uint8Array.from(bytes);
}
const OFFSETS = [undefined, 0, 1, 2, 3, 2.5, -4, 50, 2001, 3000, 1e20, Number.NaN, 100, 2000];
const LIMITS = [undefined, 0, 1, 2, -3, -1e20, 1.5, 7, 2000, 2500, 1e20, Number.NaN, -1, 10];
const dir = mkdtempSync(join(tmpdir(), "corpus-read-"));
const env = new NodeExecutionEnv({ cwd: dir });
const tool = createReadTool();
const api = { env, output: () => {}, diagnostic: () => {}, details: async () => {} } as any;
const cases: unknown[] = [];
const enc = (v: number | undefined) => (v === undefined ? null : Number.isNaN(v) ? "NaN" : v);
for (let seed = 1; seed <= 250; seed++) {
	const next = random(seed * 7919);
	const file = randomFile(next);
	writeFileSync(join(dir, "f.txt"), file);
	for (let trial = 0; trial < 4; trial++) {
		const offset = OFFSETS[Math.floor(next() * OFFSETS.length)];
		const limit = LIMITS[Math.floor(next() * LIMITS.length)];
		const args = { path: "f.txt", ...(offset === undefined ? {} : { offset }), ...(limit === undefined ? {} : { limit }) };
		let expected: unknown;
		try {
			const r = await tool.execute(args, api, {});
			expected = {
				text: (r.content ?? []).map((c: any) => c.text).join("\n"),
				isError: r.isError ?? false,
				details: r.details ?? null,
				diagnostics: r.diagnostics ?? [],
			};
		} catch (e) {
			expected = { error: (e as Error).message };
		}
		cases.push({ id: `${seed}/${trial}`, bytes: Buffer.from(file).toString("base64"), offset: enc(offset), limit: enc(limit), expected });
	}
}
console.log(JSON.stringify(cases));
