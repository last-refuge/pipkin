// Baseline measurements of the reference engine on this machine.
//   node --import <source-resolver> driver/bench.ts startup|soak [n]
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import {
	connect,
	initProject,
	isIdle,
	makeProfile,
	procInfo,
	profilePids,
	promptAndWait,
	REF,
	sleep,
	startServer,
	stopServer,
	StubProvider,
	until,
	writeJson,
} from "./lib.ts";

const mode = process.argv[2];
const pct = (xs: number[], p: number) => {
	const s = [...xs].sort((a, b) => a - b);
	return s[Math.min(s.length - 1, Math.ceil((p / 100) * s.length) - 1)]!;
};
const snapshot = (p: ReturnType<typeof makeProfile>) =>
	profilePids(p)
		.map((pid) => procInfo(pid))
		.filter((x) => x !== undefined)
		.map(({ pid, role, rssKiB, pssKiB, cmd }) => ({ pid, role, rssKiB, pssKiB, cmd: cmd.slice(0, 40) }));

if (mode === "startup") {
	const runs = Number(process.argv[3] ?? 10);
	const warmups = 2;
	const stub = new StubProvider(() => ({ text: "ready" }));
	stub.record = false;
	await stub.start();
	const rows: any[] = [];
	for (let i = 0; i < warmups + runs; i++) {
		const p = makeProfile(`bench-startup`, stub.baseUrl);
		initProject(p);
		const srv = await startServer(p);
		const s = await connect(p);
		const promptReady = s.attachedAt - srv.spawnedAt;
		const t0 = performance.now();
		await promptAndWait(s, "ping");
		const firstAnswer = performance.now() - t0;
		await sleep(2000);
		const idle = snapshot(p);
		const row = {
			run: i,
			warmup: i < warmups,
			serverReadyMs: +(srv.readyAt - srv.spawnedAt).toFixed(1),
			promptReadyMs: +promptReady.toFixed(1),
			firstPromptRoundTripMs: +firstAnswer.toFixed(1),
			idleProcesses: idle.length,
			idleRssKiB: idle.reduce((a, x) => a + x.rssKiB, 0),
			idlePssKiB: idle.reduce((a, x) => a + x.pssKiB, 0),
			idle,
		};
		rows.push(row);
		console.log(JSON.stringify({ ...row, idle: idle.map((x) => `${x.role}:${x.rssKiB}`) }));
		await s.runtime.dispose();
		const stopped = await stopServer(p, srv);
		if (stopped.leftovers.length) console.log("leftovers", stopped.leftovers);
	}
	await stub.stop();
	const measured = rows.filter((r) => !r.warmup);
	const summary = {
		runs: measured.length,
		serverReadyMs: { p50: pct(measured.map((r) => r.serverReadyMs), 50), p95: pct(measured.map((r) => r.serverReadyMs), 95), min: Math.min(...measured.map((r) => r.serverReadyMs)), max: Math.max(...measured.map((r) => r.serverReadyMs)) },
		promptReadyMs: { p50: pct(measured.map((r) => r.promptReadyMs), 50), p95: pct(measured.map((r) => r.promptReadyMs), 95), min: Math.min(...measured.map((r) => r.promptReadyMs)), max: Math.max(...measured.map((r) => r.promptReadyMs)) },
		firstPromptRoundTripMs: { p50: pct(measured.map((r) => r.firstPromptRoundTripMs), 50), p95: pct(measured.map((r) => r.firstPromptRoundTripMs), 95) },
		idleRssKiB: { p50: pct(measured.map((r) => r.idleRssKiB), 50), min: Math.min(...measured.map((r) => r.idleRssKiB)), max: Math.max(...measured.map((r) => r.idleRssKiB)) },
		idlePssKiB: { p50: pct(measured.map((r) => r.idlePssKiB), 50), min: Math.min(...measured.map((r) => r.idlePssKiB)), max: Math.max(...measured.map((r) => r.idlePssKiB)) },
	};
	console.log(JSON.stringify(summary, null, 2));
	writeJson(join(REF, "bench", `startup-${process.env.PI_TREE ? "pruned" : "full"}.json`), { summary, rows });
} else if (mode === "soak") {
	const turns = Number(process.argv[3] ?? 200);
	const stub = new StubProvider((_b, n) => ({ text: `answer ${n + 1}: ${"words ".repeat(40)}` }));
	stub.record = false;
	await stub.start();
	const p = makeProfile("bench-soak", stub.baseUrl);
	initProject(p);
	const srv = await startServer(p);
	const s = await connect(p);
	await sleep(2000);
	const samples: any[] = [{ turn: 0, procs: snapshot(p) }];
	const latencies: number[] = [];
	let peak = 0;
	let peakProcs = 0;
	const sampler = setInterval(() => {
		const snap = snapshot(p);
		peak = Math.max(peak, snap.reduce((a, x) => a + x.rssKiB, 0));
		peakProcs = Math.max(peakProcs, snap.length);
	}, 50);
	for (let t = 1; t <= turns; t++) {
		const t0 = performance.now();
		const r = await promptAndWait(s, `prompt ${t}`);
		latencies.push(performance.now() - t0);
		if (r.status !== "done") throw new Error(`turn ${t}: ${JSON.stringify(r)}`);
		if (t % 10 === 0 || t === 1) {
			const snap = snapshot(p);
			samples.push({ turn: t, procs: snap });
			console.log(t, snap.map((x) => `${x.role}:${x.rssKiB}`).join(" "), `turn ${latencies.at(-1)!.toFixed(0)}ms`);
		}
	}
	clearInterval(sampler);
	await sleep(2000);
	samples.push({ turn: "idle-after", procs: snapshot(p) });
	const total = (x: any) => x.procs.reduce((a: number, y: any) => a + y.rssKiB, 0);
	const totalPss = (x: any) => x.procs.reduce((a: number, y: any) => a + y.pssKiB, 0);
	const summary = {
		turns,
		rssKiBAtStartIdle: total(samples[0]),
		rssKiBAfterTurn1: total(samples[1]),
		rssKiBAfterLastTurn: total(samples.at(-2)),
		rssKiBIdleAfter: total(samples.at(-1)),
		pssKiBAtStartIdle: totalPss(samples[0]),
		pssKiBAfterLastTurn: totalPss(samples.at(-2)),
		rssKiBPeakSampled50ms: peak,
		maxProcessesSeen: peakProcs,
		turnLatencyMs: { p50: +pct(latencies, 50).toFixed(1), p95: +pct(latencies, 95).toFixed(1), max: +Math.max(...latencies).toFixed(1) },
		perRole: Object.fromEntries(
			["launcher/server", "coordinator", "session-worker"].map((role) => [
				role,
				samples.map((x) => ({ turn: x.turn, rssKiB: x.procs.filter((y: any) => y.role === role).reduce((a: number, y: any) => a + y.rssKiB, 0), pssKiB: x.procs.filter((y: any) => y.role === role).reduce((a: number, y: any) => a + y.pssKiB, 0) })),
			]),
		),
	};
	console.log(JSON.stringify({ ...summary, perRole: undefined }, null, 2));
	writeJson(join(REF, "bench", "soak.json"), { summary, samples });
	await s.runtime.dispose();
	await stopServer(p, srv);
	await stub.stop();
} else {
	throw new Error("usage: bench.ts startup|soak [n]");
}
process.exit(0);
