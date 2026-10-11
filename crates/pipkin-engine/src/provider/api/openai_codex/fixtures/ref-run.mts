// Differential harness (regenerate goldens with `sh regenerate.sh <pi-staged>`): runs the reference Codex SSE path (read-only import) with an injected
// fetch that captures the request and replays a scripted SSE/HTTP response. Prints JSON:
// { requests: [{url, headers, body}], message }.
import { zstdDecompressSync } from "node:zlib";

const REF = `${process.env.PI_STAGED}/packages/ai/src`;
const codex = await import(`${REF}/api/openai-codex-responses.ts`);
const transcript = await import(`${REF}/utils/transcript.ts`);

const spec = JSON.parse(await new Promise((resolve) => {
	let data = "";
	process.stdin.on("data", (c) => (data += c));
	process.stdin.on("end", () => resolve(data));
}));

const requests: unknown[] = [];
let call = 0;
const fetchImpl = async (url: string, init: { headers: Headers; body: Uint8Array | string }) => {
	const headers: Record<string, string> = {};
	for (const [k, v] of init.headers.entries()) headers[k] = v;
	let body = init.body;
	if (headers["content-encoding"] === "zstd") body = zstdDecompressSync(body as Uint8Array).toString("utf8");
	requests.push({ url, headers, body: JSON.parse(body as string) });
	const reply = spec.replies[Math.min(call, spec.replies.length - 1)];
	call++;
	if (reply.sse) {
		const enc = new TextEncoder();
		const stream = new ReadableStream({
			start(controller) {
				for (const piece of reply.sse) controller.enqueue(enc.encode(piece));
				controller.close();
			},
		});
		return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
	}
	return new Response(reply.body, { status: reply.status, headers: reply.headers ?? {} });
};

const context = transcript.normalizeContext({
	systemPrompt: spec.systemPrompt,
	messages: spec.messages,
	tools: spec.tools,
});
const s = codex.stream(spec.model, context, { ...spec.options, fetch: fetchImpl });
let message;
for await (const ev of s) {
	if (ev.type === "done") message = ev.message;
	if (ev.type === "error") message = ev.error;
}
// Wall-clock timestamps are the only nondeterministic field; drop them.
if (message) delete message.timestamp;
console.log(JSON.stringify({ requests, message }, null, 1));
