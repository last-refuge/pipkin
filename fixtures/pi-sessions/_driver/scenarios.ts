// Drives one reference scenario end to end and writes its outputs to ref-run/out/<scenario>/.
//   node --import <source-resolver> driver/scenarios.ts <scenario>
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
	CTX,
	connect,
	Gate,
	git,
	hasTool,
	inbox,
	initProject,
	isCompaction,
	isIdle,
	isToolResultTurn,
	killProfile,
	lastUserText,
	live,
	makeProfile,
	procInfo,
	profilePids,
	prompt,
	promptAndWait,
	REF,
	type Profile,
	type Reply,
	type Script,
	type Session,
	sessionDir,
	sleep,
	startServer,
	stopServer,
	StubProvider,
	until,
	writeJson,
	type RunningServer,
} from "./lib.ts";

const name = process.argv[2];
if (!name) throw new Error("usage: scenarios.ts <scenario>");
const OUT = join(REF, "out", name);
rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });

const notes: string[] = [];
let current: Profile | undefined;
const note = (s: string) => {
	notes.push(s);
	console.log(`[${name}] ${s}`);
};

interface Ctx {
	p: Profile;
	stub: StubProvider;
	srv: RunningServer;
	s: Session;
}

async function setup(script: Script, settings?: unknown, files?: Record<string, string>): Promise<Ctx> {
	const stub = new StubProvider(script, join(OUT, "requests.jsonl"));
	await stub.start();
	const p = makeProfile(name, stub.baseUrl, settings);
	current = p;
	initProject(p, files);
	const srv = await startServer(p);
	const s = await connect(p);
	note(`session ${s.sessionId}; server ready ${(srv.readyAt - srv.spawnedAt).toFixed(0)} ms, attached ${(s.attachedAt - srv.spawnedAt).toFixed(0)} ms after spawn`);
	return { p, stub, srv, s };
}

async function dumpViews(c: Ctx, suffix = "") {
	writeJson(join(OUT, `view${suffix}.json`), c.s.transcript.state.value);
	// Whole fork-aware history, oldest first.
	const all: unknown[] = [];
	let before: number | null = null;
	for (;;) {
		const page = await c.s.history.page({ before, limit: 200 }, CTX);
		all.unshift(...[...page.entries].reverse());
		if (!page.more || page.entries.length === 0) break;
		before = (page.entries.at(-1) as any).id;
	}
	writeJson(join(OUT, `history${suffix}.json`), all);
	try {
		const children = await c.s.subagents.list(CTX);
		if (children.length > 0) {
			writeJson(join(OUT, `subagents${suffix}.json`), children);
			for (const child of children) writeJson(join(OUT, `subagent-${child.conversationId}-view${suffix}.json`), await c.s.subagents.view(child.conversationId, CTX));
		}
	} catch (e) {
		note(`subagents.list failed: ${(e as Error).message}`);
	}
	try {
		writeJson(join(OUT, `models${suffix}.json`), c.s.models.state.value);
	} catch {}
}

async function finish(c: Ctx) {
	await dumpViews(c);
	const sid = c.s.sessionId;
	await c.s.runtime.dispose();
	const { code, leftovers } = await stopServer(c.p, c.srv);
	note(`server exit code ${code}; leftover profile processes after stop: ${JSON.stringify(leftovers.map((pid) => procInfo(pid)))}`);
	await c.stub.stop();
	const dir = sessionDir(c.p, sid);
	const listing = readdirSync(dir).map((f) => `${f} ${statSync(join(dir, f)).size}`);
	note(`session dir after stop: ${listing.join(", ")}`);
	const dest = join(OUT, "session");
	mkdirSync(dest, { recursive: true });
	for (const f of ["meta.json", "session.sqlite"]) copyFileSync(join(dir, f), join(dest, f));
	for (const f of ["session.sqlite-wal", "session.sqlite-shm"]) {
		if (existsSync(join(dir, f)) && statSync(join(dir, f)).size > 0) note(`WARNING: ${f} present and non-empty after stop`);
	}
	writeFileSync(join(OUT, "notes.txt"), `${notes.join("\n")}\n`);
	writeJson(join(OUT, "run.json"), { scenario: name, sessionId: sid, project: c.p.project, agentDir: c.p.agentDir, serverDir: c.p.serverDir });
}

const text = (t: string): Reply => ({ text: t });

const scenarios: Record<string, () => Promise<void>> = {
	// (a) plain text replies, multi-turn
	async "a-text-multiturn"() {
		const c = await setup((_b, n) => text(`Answer ${n + 1}: plain text from the scripted model.`));
		for (const q of ["Hello, who are you?", "Tell me a second thing.", "And a third, please."]) {
			const r = await promptAndWait(c.s, q);
			note(`prompt ${JSON.stringify(q)} -> ${JSON.stringify(r)}`);
		}
		await finish(c);
	},

	// (b) write, read, edit, bash with a git diff
	async "b-tools-diff"() {
		const steps: Reply[] = [
			{ text: "I'll create the notes file. ", tool: { name: "write", args: { path: "notes.txt", content: "alpha\nbeta\ngamma\n" } } },
			{ tool: { name: "read", args: { path: "README.md" } } },
			{ text: "Now editing the README. ", tool: { name: "edit", args: { path: "README.md", edits: [{ oldText: "Fixture project", newText: "Fixture project (edited)" }] } } },
			{ tool: { name: "bash", args: { command: "git status --short && git diff" } } },
			{ text: "Done: wrote notes.txt, edited README.md, and showed the diff." },
		];
		let step = 0;
		const c = await setup(() => steps[Math.min(step++, steps.length - 1)]!, undefined, {
			"README.md": "# Fixture project\n\nA small project for reference fixtures.\n",
		});
		const r = await promptAndWait(c.s, "Create notes.txt, look at the README, edit its title and show me the diff.");
		note(`prompt -> ${JSON.stringify(r)}`);
		note(`git status after run:\n${git(c.p.project, "status", "--short")}`);
		writeFileSync(join(OUT, "project-diff.txt"), git(c.p.project, "diff"));
		await finish(c);
	},

	// (c) Stop during a held model reply, then a new prompt
	async "c-stop-then-prompt"() {
		const gate = new Gate();
		const c = await setup((_b, n) => (n === 0 ? { text: "never delivered", hold: gate.promise } : text("Fresh answer after the stop.")));
		const { operationId } = await prompt(c.s, "Take your time on this one.");
		await until("model asked", () => c.stub.chatRequests().length >= 1);
		await until("running", () => live(c.s).run !== undefined);
		await sleep(200);
		await c.s.agent.abort(CTX);
		const r1 = await c.s.agent.waitForPrompt(operationId, CTX);
		note(`stopped prompt settled: ${JSON.stringify(r1)}`);
		await until("idle after stop", () => isIdle(c.s));
		gate.open();
		const r2 = await promptAndWait(c.s, "Next question after stopping.");
		note(`next prompt -> ${JSON.stringify(r2)}`);
		await finish(c);
	},

	// (d) steer during a held first turn (tool call), plus a follow-up queued behind the run
	async "d-steer-followup"() {
		const gate = new Gate();
		const c = await setup((_b, n) =>
			n === 0
				? { text: "Working. ", tool: { name: "write", args: { path: "notes.txt", content: "one\ntwo\n" } }, hold: gate.promise }
				: n === 1
					? text("Done, and I used tabs as you asked.")
					: text(`Follow-up answered (turn ${n + 1}).`),
		);
		const { operationId } = await prompt(c.s, "Edit the notes.");
		await until("model asked", () => c.stub.chatRequests().length >= 1);
		await until("running", () => live(c.s).run !== undefined);
		const steer = await c.s.agent.steer({ message: "Use tabs.", images: null }, CTX);
		note(`steer -> ${JSON.stringify(steer)}`);
		const follow = await c.s.agent.followUp({ message: "Afterwards, summarize what changed.", images: null }, CTX);
		note(`followUp -> ${JSON.stringify(follow)}`);
		await sleep(300);
		writeJson(join(OUT, "inbox-while-running.json"), inbox(c.s));
		gate.open();
		const r = await c.s.agent.waitForPrompt(operationId, CTX);
		note(`first prompt -> ${JSON.stringify(r)}`);
		await until("all runs done", () => c.stub.chatRequests().length >= 3 && isIdle(c.s), 30000);
		await sleep(300);
		await finish(c);
	},

	// (e) manual compaction after several turns, then one more prompt
	async "e-compaction"() {
		const c = await setup(
			(b, n) =>
				isCompaction(b)
					? text("## Goal\nScripted summary of the earlier turns.\n\n## Progress\n- Turns 1-3 discussed topics one to three.\n\n## Next Steps\n- Continue.")
					: text(`Answer ${n + 1} about ${lastUserText(b).split("\n")[0]}.`),
			{ compaction: { keepRecentTokens: 60 } },
		);
		for (let i = 1; i <= 4; i++) {
			const r = await promptAndWait(c.s, `Topic ${i}: ${"some words about this topic ".repeat(8)}`);
			note(`prompt ${i} -> ${JSON.stringify(r)}`);
		}
		const comp = await c.s.agent.compact({ customInstructions: null }, CTX);
		note(`compact -> ${JSON.stringify(comp)}`);
		await until("compaction requested", () => c.stub.chatRequests().some((r) => isCompaction(r.body)));
		await until("compaction done", () => isIdle(c.s), 30000);
		await sleep(300);
		await dumpViews(c, "-after-compaction");
		const r = await promptAndWait(c.s, "What were we talking about?");
		note(`post-compaction prompt -> ${JSON.stringify(r)}`);
		await finish(c);
	},

	// (f) a subagent call that completes
	async "f-subagent"() {
		const c = await setup((b, n) => {
			const child = !hasTool(b, "subagent");
			if (child) return text("Child answer: the subagent counted three files.");
			if (isToolResultTurn(b)) return text("The subagent reports three files. Done.");
			return { text: "Delegating. ", tool: { name: "subagent", args: { task: "Count the files in the project and report the number." } } };
		});
		await c.s.subagents.setEnabled(true, CTX);
		await until("subagents enabled", () => (c.s.subagents.configuration.value as any)?.enabled === true);
		note(`subagents enabled: ${JSON.stringify(c.s.subagents.configuration.value)}`);
		const r = await promptAndWait(c.s, "Ask a subagent to count the files.");
		note(`prompt -> ${JSON.stringify(r)}`);
		await sleep(300);
		await finish(c);
	},

	// (g) model and thinking changes between prompts
	async "g-model-thinking"() {
		const c = await setup((b, n) =>
			b?.model === "scripted-think"
				? { reasoning: `Thinking about request ${n + 1}.`, text: `Answer ${n + 1} from ${b.model} (reasoning_effort=${b.reasoning_effort ?? "none"}).` }
				: text(`Answer ${n + 1} from ${b?.model}.`),
		);
		note(`models state before: ${JSON.stringify((c.s.models.state.value as any)?.configuration)}`);
		await promptAndWait(c.s, "First, on the default model.");
		await c.s.models.select({ provider: "stub", modelId: "scripted-think" }, CTX);
		await until("model selected", () => (c.s.models.state.value as any)?.configuration?.model?.modelId === "scripted-think");
		note(`after select: ${JSON.stringify((c.s.models.state.value as any)?.configuration)}; levels ${JSON.stringify(await c.s.models.getThinkingLevels(CTX))}`);
		await promptAndWait(c.s, "Second, on the reasoning model.");
		await c.s.models.selectThinking("high", CTX);
		await until("thinking high", () => (c.s.models.state.value as any)?.configuration?.thinkingLevel === "high");
		note(`after selectThinking: ${JSON.stringify((c.s.models.state.value as any)?.configuration)}`);
		await promptAndWait(c.s, "Third, with high thinking.");
		await finish(c);
	},

	// (h) kill the session worker mid-tool (bash sleep), then reopen
	async "h-worker-killed-mid-tool"() {
		const c = await setup((b, n) =>
			isToolResultTurn(b) ? text(`Recovered after the interruption (turn ${n + 1}).`) : { text: "Running the slow command. ", tool: { name: "bash", args: { command: "sleep 15; echo finished" } } },
		);
		const { operationId } = await prompt(c.s, "Run the slow command.");
		note(`prompt operation ${operationId}`);
		await until("tool running", () => (live(c.s).tools ?? []).some((t: any) => t.status === "running"), 30000);
		await sleep(500);
		writeJson(join(OUT, "view-before-kill.json"), c.s.transcript.state.value);
		const procs = profilePids(c.p).map((pid) => procInfo(pid)!).filter(Boolean);
		note(`processes before kill: ${JSON.stringify(procs.map(({ pid, role, cmd }) => ({ pid, role, cmd: cmd.slice(0, 160) })))}`);
		const workers = procs.filter((x) => x.role === "session-worker");
		if (workers.length !== 1) throw new Error(`expected one worker, found ${workers.length}`);
		process.kill(workers[0]!.pid, "SIGKILL");
		note(`SIGKILLed session worker ${workers[0]!.pid}`);
		// The tool's shell (sleep) may survive its worker; kill it too so it cannot write afterwards.
		await sleep(300);
		for (const x of profilePids(c.p).map((pid) => procInfo(pid)!).filter(Boolean)) {
			if (x.cmd.includes("sleep 15")) {
				process.kill(x.pid, "SIGKILL");
				note(`killed orphaned tool process ${x.pid}: ${x.cmd}`);
			}
		}
		await sleep(500);
		const sid = c.s.sessionId;
		try {
			await c.s.runtime.dispose();
		} catch (e) {
			note(`client dispose after kill: ${(e as Error).message}`);
		}
		// Reopen: a fresh client attaches the same Session; the server spawns a new worker.
		const s2 = await connect(c.p, sid);
		c.s = s2;
		note(`reattached ${sid}`);
		await until("resumed run settles", () => isIdle(s2) && c.stub.chatRequests().length >= 2, 30000).catch((e) => note(`did not settle: ${e.message}`));
		const r = await s2.agent.waitForPrompt(operationId, CTX).catch((e) => ({ error: String(e) }));
		note(`original prompt settled: ${JSON.stringify(r)}`);
		const procs2 = profilePids(c.p).map((pid) => procInfo(pid)!).filter(Boolean);
		note(`processes after reopen: ${JSON.stringify(procs2.map(({ pid, role }) => ({ pid, role })))}`);
		await sleep(300);
		await finish(c);
	},
};

const run = scenarios[name];
if (!run) throw new Error(`unknown scenario ${name}; known: ${Object.keys(scenarios).join(", ")}`);
try {
	await run();
	console.log(`[${name}] OK`);
	process.exit(0);
} catch (e) {
	console.error(`[${name}] FAILED`, e);
	if (current) killProfile(current);
	writeFileSync(join(OUT, "notes.txt"), `${notes.join("\n")}\nFAILED: ${(e as Error).stack}\n`);
	process.exit(1);
}
