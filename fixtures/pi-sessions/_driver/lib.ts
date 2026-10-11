// Shared harness for driving the reference Pi experimental server against a scripted,
// local OpenAI-compatible stub provider. No real credentials, no network providers.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync, appendFileSync } from "node:fs";
import http from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { openClientRuntime, type ClientRuntime, type ClientRuntimeServer } from "../pi/packages/coding-agent/src/experimental/client-runtime.ts";
import { AgentController } from "../pi/packages/coding-agent/src/experimental/services/agent-controller.ts";
import { History } from "../pi/packages/coding-agent/src/experimental/services/history.ts";
import { Models } from "../pi/packages/coding-agent/src/experimental/services/models.ts";
import { PresentationPlugins } from "../pi/packages/coding-agent/src/experimental/services/plugins.ts";
import { SessionDirectory, SessionManagement } from "../pi/packages/coding-agent/src/experimental/services/sessions.ts";
import { Subagents } from "../pi/packages/coding-agent/src/experimental/services/subagents.ts";
import { Transcript } from "../pi/packages/coding-agent/src/experimental/services/transcript.ts";

export const CTX = BACKGROUND_CONTEXT;
export const HERE = dirname(fileURLToPath(import.meta.url));
export const REF = join(HERE, "..");
export const PI = process.env.PI_TREE ?? join(REF, "pi");
export const NODE = process.execPath;
export const RESOLVER = pathToFileURL(join(PI, "packages/coding-agent/src/experimental/source-resolver.ts")).href;
export const CLI = join(PI, "packages/coding-agent/src/experimental/cli.ts");
/** Unix socket paths are limited to ~108 bytes, so socket dirs live under a short root. */
export const SOCK_ROOT = "/tmp/claude-1000/pkr";
export const SERVER_ID = "5f0c7b1e-2d4a-4f6b-9a3e-1c8d7e6f5a40";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function until<T>(what: string, f: () => T | undefined | false | Promise<T | undefined | false>, timeoutMs = 30000): Promise<T> {
	const deadline = Date.now() + timeoutMs;
	for (;;) {
		const v = await f();
		if (v) return v as T;
		if (Date.now() > deadline) throw new Error(`timed out waiting for: ${what}`);
		await sleep(20);
	}
}

// ---------------------------------------------------------------------------------------------
// Stub provider
// ---------------------------------------------------------------------------------------------

export interface Reply {
	text?: string;
	reasoning?: string;
	tool?: { name: string; args: unknown };
	/** Hold the response until this resolves (or the client disconnects). */
	hold?: Promise<void>;
	/** Never answer (keep-alive) until the client goes away. */
	hang?: boolean;
}
export type Script = (body: any, n: number) => Reply;

export class Gate {
	open!: () => void;
	readonly promise = new Promise<void>((r) => (this.open = r));
}

const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
const chunk = (delta: unknown, finish: string | null = null, withUsage = false) =>
	`data: ${JSON.stringify({
		id: "stub-1",
		object: "chat.completion.chunk",
		created: 0,
		model: "scripted",
		choices: [{ index: 0, delta, finish_reason: finish }],
		...(withUsage ? { usage } : {}),
	})}\n\n`;

export function events(reply: Reply, n: number): string[] {
	const out = [chunk({ role: "assistant", content: "" })];
	if (reply.reasoning) out.push(chunk({ reasoning_content: reply.reasoning }));
	if (reply.tool) {
		if (reply.text) out.push(chunk({ content: reply.text }));
		out.push(
			chunk({
				tool_calls: [
					{ index: 0, id: `call_${n}`, type: "function", function: { name: reply.tool.name, arguments: JSON.stringify(reply.tool.args) } },
				],
			}),
		);
		out.push(chunk({}, "tool_calls", true));
	} else {
		out.push(chunk({ content: reply.text ?? "" }));
		out.push(chunk({}, "stop", true));
	}
	out.push("data: [DONE]\n\n");
	return out;
}

export class StubProvider {
	readonly requests: any[] = [];
	readonly server: http.Server;
	port = 0;
	record = true;
	count = 0;
	script: Script;
	logPath?: string;
	constructor(script: Script, logPath?: string) {
		this.script = script;
		this.logPath = logPath;
		this.server = http.createServer((req, res) => {
			let body = "";
			req.on("data", (d) => (body += d));
			req.on("end", async () => {
				if (req.method === "GET") {
					const b = JSON.stringify({ object: "list", data: [{ id: "scripted", object: "model" }] });
					res.writeHead(200, { "content-type": "application/json" });
					res.end(b);
					return;
				}
				let parsed: any = null;
				try {
					parsed = JSON.parse(body);
				} catch {}
				const n = this.count++;
				const headers = { ...req.headers };
				const entry = { n, method: req.method, path: req.url, headers, body: parsed };
				if (this.record) {
					this.requests.push(entry);
					if (this.logPath) appendFileSync(this.logPath, `${JSON.stringify(entry)}\n`);
				}
				let gone = false;
				res.on("close", () => (gone = true));
				const reply = this.script(parsed, n);
				if (reply.hold) {
					await Promise.race([reply.hold, new Promise<void>((r) => res.on("close", () => r()))]);
				}
				if (gone) return;
				res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
				if (reply.hang) {
					res.write(chunk({ role: "assistant", content: "" }));
					const t = setInterval(() => res.write(": keep-alive\n\n"), 200);
					res.on("close", () => clearInterval(t));
					return;
				}
				for (const e of events(reply, n)) res.write(e);
				res.end();
			});
		});
	}
	async start(): Promise<void> {
		await new Promise<void>((r) => this.server.listen(0, "127.0.0.1", () => r()));
		this.port = (this.server.address() as any).port;
	}
	get baseUrl() {
		return `http://127.0.0.1:${this.port}/v1`;
	}
	chatRequests() {
		return this.requests.filter((r) => String(r.path).endsWith("/chat/completions"));
	}
	async stop() {
		this.server.closeAllConnections();
		await new Promise<void>((r) => this.server.close(() => r()));
	}
}

// helpers on request bodies
export const lastMessage = (body: any) => body?.messages?.at(-1);
export const isToolResultTurn = (body: any) => lastMessage(body)?.role === "tool";
export const lastUserText = (body: any): string => {
	const m = [...(body?.messages ?? [])].reverse().find((m: any) => m.role === "user");
	if (!m) return "";
	return typeof m.content === "string" ? m.content : (m.content ?? []).map((p: any) => p.text ?? "").join("");
};
export const isCompaction = (body: any) =>
	(body?.messages ?? []).some((m: any) => m.role === "system" && String(m.content).includes("context summarization assistant"));
export const hasTool = (body: any, name: string) => (body?.tools ?? []).some((t: any) => t.function?.name === name);

// ---------------------------------------------------------------------------------------------
// Profile layout and server process
// ---------------------------------------------------------------------------------------------

export interface Profile {
	name: string;
	root: string; // scratch root for this run
	agentDir: string;
	serverDir: string; // short, for sockets
	project: string;
	home: string;
	log: string;
}

export function makeProfile(name: string, baseUrl: string, settings?: unknown): Profile {
	const root = join(REF, "runs", name);
	rmSync(root, { recursive: true, force: true });
	const serverDir = join(SOCK_ROOT, name);
	rmSync(serverDir, { recursive: true, force: true });
	const p: Profile = {
		name,
		root,
		agentDir: join(root, "agent"),
		serverDir,
		project: join(root, "project"),
		home: join(root, "home"),
		log: join(root, "server.log"),
	};
	for (const d of [p.agentDir, p.project, p.home]) mkdirSync(d, { recursive: true });
	mkdirSync(serverDir, { recursive: true, mode: 0o700 });
	writeFileSync(
		join(p.agentDir, "models.json"),
		JSON.stringify(
			{
				providers: {
					stub: {
						baseUrl,
						api: "openai-completions",
						apiKey: "stub",
						models: [
							{ id: "scripted", name: "Scripted stub", input: ["text", "image"] },
							{ id: "scripted-think", name: "Scripted stub (reasoning)", reasoning: true, input: ["text"] },
						],
					},
				},
			},
			null,
			2,
		),
	);
	if (settings !== undefined) writeFileSync(join(p.agentDir, "settings.json"), JSON.stringify(settings, null, 2));
	return p;
}

export function git(dir: string, ...args: string[]) {
	return execFileSync("git", ["-c", "user.name=Pipkin Fixture", "-c", "user.email=fixture@example.invalid", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false", ...args], {
		cwd: dir,
		env: { PATH: process.env.PATH!, HOME: join(REF, "home"), GIT_CONFIG_NOSYSTEM: "1" },
		encoding: "utf8",
	});
}

export function initProject(p: Profile, files: Record<string, string> = { "README.md": "# Fixture project\n" }) {
	git(p.project, "init", "-q");
	for (const [f, c] of Object.entries(files)) writeFileSync(join(p.project, f), c);
	git(p.project, "add", "-A");
	git(p.project, "commit", "-q", "-m", "initial");
}

export function serverEnv(p: Profile): NodeJS.ProcessEnv {
	return {
		PATH: `${dirname(NODE)}:/usr/local/bin:/usr/bin:/bin`,
		HOME: p.home,
		LANG: "C.UTF-8",
		TZ: "UTC",
		PI_EXPERIMENTAL: "1",
		PI_OFFLINE: "1",
		PI_SERVER_DIR: p.serverDir,
		PI_SERVER_ID: SERVER_ID,
		PI_CODING_AGENT_DIR: p.agentDir,
		GIT_CONFIG_NOSYSTEM: "1",
	};
}

export interface RunningServer {
	proc: ChildProcess;
	spawnedAt: number;
	readyAt: number;
	exited: Promise<number | null>;
}

export async function startServer(p: Profile, model = "scripted"): Promise<RunningServer> {
	const spawnedAt = performance.now();
	const out = (await import("node:fs")).openSync(p.log, "a");
	const proc = spawn(NODE, ["--import", RESOLVER, CLI, "server", "--provider", "stub", "--model", model], {
		cwd: p.root,
		env: serverEnv(p),
		stdio: ["ignore", out, out],
	});
	const exited = new Promise<number | null>((r) => proc.on("exit", (code) => r(code)));
	await until("server prints its socket", () => existsSync(p.log) && readFileSync(p.log, "utf8").includes("Socket:"), 60000);
	return { proc, spawnedAt, readyAt: performance.now(), exited };
}

/** All processes carrying this profile's identity (like Pipkin's engine::profile_pids). */
export function profilePids(p: Profile): number[] {
	const out: number[] = [];
	for (const name of readdirSync("/proc")) {
		const pid = Number(name);
		if (!Number.isInteger(pid) || pid === process.pid) continue;
		let env: string;
		try {
			env = readFileSync(`/proc/${pid}/environ`, "latin1");
		} catch {
			continue;
		}
		const vars = env.split("\0");
		if (vars.includes(`PI_SERVER_DIR=${p.serverDir}`) && vars.includes(`PI_SERVER_ID=${SERVER_ID}`)) out.push(pid);
	}
	return out;
}

export function procInfo(pid: number): { pid: number; role: string; cmd: string; rssKiB: number; pssKiB: number } | undefined {
	try {
		const env = readFileSync(`/proc/${pid}/environ`, "latin1").split("\0");
		const role = env.find((v) => v.startsWith("__PI_INTERNAL_SPAWN="))?.split("=")[1] ?? "launcher/server";
		const cmd = readFileSync(`/proc/${pid}/cmdline`, "latin1").split("\0").join(" ").trim();
		const status = readFileSync(`/proc/${pid}/status`, "latin1");
		const rss = Number(/VmRSS:\s+(\d+)/.exec(status)?.[1] ?? 0);
		let pss = 0;
		try {
			pss = Number(/Pss:\s+(\d+)/.exec(readFileSync(`/proc/${pid}/smaps_rollup`, "latin1"))?.[1] ?? 0);
		} catch {}
		return { pid, role, cmd, rssKiB: rss, pssKiB: pss };
	} catch {
		return undefined;
	}
}

export async function stopServer(p: Profile, s: RunningServer): Promise<{ code: number | null; leftovers: number[] }> {
	s.proc.kill("SIGTERM");
	const code = await Promise.race([s.exited, sleep(20000).then(() => "timeout" as const)]);
	if (code === "timeout") throw new Error("server did not exit on SIGTERM");
	// Give detached children (coordinator, workers) a moment to wind down.
	let leftovers: number[] = [];
	for (let i = 0; i < 100; i++) {
		leftovers = profilePids(p);
		if (leftovers.length === 0) break;
		await sleep(50);
	}
	return { code, leftovers };
}

export function killProfile(p: Profile) {
	for (const pid of profilePids(p)) {
		try {
			process.kill(pid, "SIGKILL");
		} catch {}
	}
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

export interface Session {
	runtime: ClientRuntime;
	server: ClientRuntimeServer;
	sessionId: string;
	management: SessionManagement;
	agent: AgentController;
	models: Models;
	transcript: Transcript;
	subagents: Subagents;
	history: History;
	attachedAt: number;
}

export async function connect(p: Profile, sessionId?: string, cwd?: string): Promise<Session> {
	const runtime = await openClientRuntime({ command: "client" } as any, { directory: p.serverDir });
	const server = runtime.servers[0]!;
	const serverServices = server.server.open({
		services: [SessionDirectory, SessionManagement, PresentationPlugins],
		assertAccess() {},
		onError() {},
	});
	const sessionServices = server.session.open({
		services: [Models, AgentController, Transcript, Subagents, History],
		assertAccess() {},
		onError() {},
	});
	const remote = serverServices.use(SessionManagement);
	const plugins = serverServices.use(PresentationPlugins);
	await serverServices.ready(CTX);
	if (sessionId === undefined) sessionId = (await remote.create({ cwd: cwd ?? p.project }, CTX)).sessionId;
	await plugins.prepareSession({ sessionId, packagePaths: null }, CTX);
	await remote.attach(sessionId, CTX);
	await server.session.whenAttached(sessionId, CTX);
	await sessionServices.ready(CTX);
	const transcript = sessionServices.use(Transcript);
	await until("transcript hydrated", () => transcript.state.value !== undefined);
	return {
		runtime,
		server,
		sessionId,
		management: remote,
		agent: sessionServices.use(AgentController),
		models: sessionServices.use(Models),
		transcript,
		subagents: sessionServices.use(Subagents),
		history: sessionServices.use(History),
		attachedAt: performance.now(),
	};
}

export const live = (s: Session): any => (s.transcript.state.value as any)?.docs?.["pi.live"] ?? {};
export const inbox = (s: Session): any => (s.transcript.state.value as any)?.docs?.["pi.inbox"] ?? {};
export const isIdle = (s: Session) => {
	const l = live(s);
	return l.run === undefined && (l.compactions ?? []).length === 0;
};

export async function prompt(s: Session, message: string): Promise<{ operationId: string }> {
	const r = await s.agent.prompt({ message, images: null }, CTX);
	if (!r.accepted) throw new Error(`prompt rejected: ${JSON.stringify(r.error)}`);
	return { operationId: r.operationId };
}

export async function promptAndWait(s: Session, message: string) {
	const { operationId } = await prompt(s, message);
	const result = await s.agent.waitForPrompt(operationId, CTX);
	await until("idle", () => isIdle(s));
	return result;
}

export function sessionDir(p: Profile, sessionId: string) {
	return join(p.agentDir, "experimental", "sessions", sessionId);
}

export function writeJson(path: string, v: unknown) {
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, `${JSON.stringify(v, null, 2)}\n`);
}
