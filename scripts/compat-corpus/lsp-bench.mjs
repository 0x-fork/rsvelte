#!/usr/bin/env node
/**
 * Language-server benchmarks: `rsvelte-language-server` against the real
 * `svelte-language-server`, over the same stdio protocol the parity gate uses.
 *
 * Four measurements, in the order a user feels them:
 *   1. COLD START — spawn to the `initialize` response.
 *   2. TIME TO FIRST DIAGNOSTIC — spawn to the first `publishDiagnostics` for a
 *      freshly opened component. This is the one that decides whether an editor
 *      feels alive, and it is not the sum of the others: both servers load a
 *      TypeScript program lazily behind it.
 *   3. HOVER / COMPLETION LATENCY — p50/p90/p99 over a fixed position set, after
 *      a warmup that excludes program load from the sample.
 *   4. MEMORY — peak RSS of the server process and every child it spawned
 *      (rsvelte's tsgo child is a separate process, so a single-pid reading
 *      would report a number that is not the cost of running the server).
 *
 * This is a benchmark, not a gate: it asserts nothing and never fails a build.
 * Numbers from different machines are not comparable; run both sides in one
 * invocation, which is what this script does.
 *
 * Usage:
 *   node scripts/compat-corpus/lsp-bench.mjs                  # default project
 *   node scripts/compat-corpus/lsp-bench.mjs --project <dir>  # any workspace
 *   node scripts/compat-corpus/lsp-bench.mjs --runs 5 --json out.json
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { LspClient, clientCapabilities } from './lsp/client.mjs';
import { languageId, samplePositions } from './lsp/population.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '../..');
const ORACLE_DIR = path.join(__dirname, 'lsp-oracle');

const args = process.argv.slice(2);
const RUNS = num('--runs', 3);
const SAMPLES = num('--samples', 60);
const JSON_OUT = args.includes('--json') ? args[args.indexOf('--json') + 1] : null;
const PROJECT = args.includes('--project')
	? path.resolve(args[args.indexOf('--project') + 1])
	: defaultProject();

function num(flag, fallback) {
	if (!args.includes(flag)) return fallback;
	const value = Number(args[args.indexOf(flag) + 1]);
	if (!Number.isFinite(value) || value <= 0) die(`${flag} needs a positive number`);
	return value;
}

function die(msg) {
	console.error(`[lsp-bench] ${msg}`);
	process.exit(2);
}

/** The largest real project available in this checkout, falling back to a fixture. */
function defaultProject() {
	for (const candidate of [
		'submodules/cmsaasstarter',
		'submodules/skeleton',
		'submodules/bits-ui/packages/bits-ui/src',
		'submodules/flowbite-svelte/src/lib',
	]) {
		const p = path.join(ROOT, candidate);
		if (fs.existsSync(p)) return p;
	}
	return path.join(ROOT, 'compatibility/lsp-fixtures/basic/project');
}

function components(root, limit) {
	const out = [];
	const walk = (dir) => {
		let entries;
		try {
			entries = fs.readdirSync(dir, { withFileTypes: true });
		} catch {
			return;
		}
		for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
			if (entry.name.startsWith('.') || entry.name === 'node_modules') continue;
			const full = path.join(dir, entry.name);
			if (entry.isDirectory()) walk(full);
			else if (entry.name.endsWith('.svelte')) out.push(full);
		}
	};
	walk(root);
	return out.slice(0, limit);
}

function fileUri(p) {
	return 'file://' + p.split(path.sep).join('/');
}

/** Peak RSS in MiB of `pid` and every descendant, sampled through `ps`. */
function treeRss(pid) {
	const pids = [pid];
	for (let i = 0; i < pids.length; i++) {
		try {
			const children = execFileSync('pgrep', ['-P', String(pids[i])], { encoding: 'utf8' });
			for (const line of children.split('\n')) {
				const child = Number(line.trim());
				if (child) pids.push(child);
			}
		} catch {
			// `pgrep` exits 1 when a process has no children.
		}
	}
	let total = 0;
	for (const p of pids) {
		try {
			const rss = execFileSync('ps', ['-o', 'rss=', '-p', String(p)], { encoding: 'utf8' });
			total += Number(rss.trim()) || 0;
		} catch {
			// The process exited between listing and reading; it contributes nothing.
		}
	}
	return total / 1024;
}

function percentile(sorted, p) {
	if (sorted.length === 0) return null;
	const index = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
	return sorted[index];
}

function stats(samples) {
	const sorted = [...samples].sort((a, b) => a - b);
	return {
		n: sorted.length,
		p50: percentile(sorted, 50),
		p90: percentile(sorted, 90),
		p99: percentile(sorted, 99),
		max: sorted[sorted.length - 1] ?? null,
	};
}

function spawnServer(side, workspace, oracleBin, rsvelteBin) {
	return side === 'official'
		? new LspClient({
				label: 'official',
				command: process.execPath,
				args: [oracleBin, '--stdio'],
				cwd: workspace,
				timeoutMs: 120_000,
			})
		: new LspClient({
				label: 'rsvelte',
				command: rsvelteBin,
				args: ['--stdio'],
				cwd: workspace,
				timeoutMs: 120_000,
			});
}

async function initialize(client, workspace) {
	await client.request(
		'initialize',
		{
			processId: process.pid,
			clientInfo: { name: 'rsvelte-lsp-bench', version: '1' },
			rootUri: fileUri(workspace),
			rootPath: workspace,
			workspaceFolders: [{ uri: fileUri(workspace), name: path.basename(workspace) }],
			capabilities: clientCapabilities(),
			initializationOptions: { isTrusted: true },
		},
		{ timeoutMs: 120_000 }
	);
	client.notify('initialized', {});
}

async function measure(side, { workspace, oracleBin, rsvelteBin, files }) {
	const coldStart = [];
	for (let i = 0; i < RUNS; i++) {
		const started = process.hrtime.bigint();
		const client = spawnServer(side, workspace, oracleBin, rsvelteBin);
		await initialize(client, workspace);
		coldStart.push(Number(process.hrtime.bigint() - started) / 1e6);
		await client.shutdown();
	}

	const firstDiagnostic = [];
	for (let i = 0; i < RUNS; i++) {
		const file = files[i % files.length];
		const uri = fileUri(file);
		const started = process.hrtime.bigint();
		const client = spawnServer(side, workspace, oracleBin, rsvelteBin);
		await initialize(client, workspace);
		client.notify('textDocument/didOpen', {
			textDocument: {
				uri,
				languageId: languageId(file),
				version: 1,
				text: fs.readFileSync(file, 'utf8'),
			},
		});
		const got = await client.waitFor(() => client.diagnostics.has(uri), {
			timeoutMs: 120_000,
			intervalMs: 5,
		});
		firstDiagnostic.push(got ? Number(process.hrtime.bigint() - started) / 1e6 : NaN);
		await client.shutdown();
	}

	// One long-lived server for the latency and memory numbers: an editor keeps
	// the process, so measuring a fresh one per request would price program load
	// into every sample.
	const client = spawnServer(side, workspace, oracleBin, rsvelteBin);
	await initialize(client, workspace);
	const opened = files.slice(0, 25);
	for (const file of opened) {
		client.notify('textDocument/didOpen', {
			textDocument: {
				uri: fileUri(file),
				languageId: languageId(file),
				version: 1,
				text: fs.readFileSync(file, 'utf8'),
			},
		});
	}
	await client.waitFor(() => opened.every((f) => client.diagnostics.has(fileUri(f))), {
		timeoutMs: 180_000,
		intervalMs: 50,
	});

	const hover = [];
	const completion = [];
	let peakRss = 0;
	for (const file of opened) {
		const text = fs.readFileSync(file, 'utf8');
		const uri = fileUri(file);
		for (const position of samplePositions(text, Math.ceil(SAMPLES / opened.length))) {
			for (const [method, into, extra] of [
				['textDocument/hover', hover, {}],
				['textDocument/completion', completion, { context: { triggerKind: 1 } }],
			]) {
				const started = process.hrtime.bigint();
				try {
					await client.request(method, { textDocument: { uri }, position, ...extra });
					into.push(Number(process.hrtime.bigint() - started) / 1e6);
				} catch {
					// A refused or timed-out request is not a latency sample.
				}
			}
		}
		peakRss = Math.max(peakRss, treeRss(client.proc.pid));
	}
	await client.shutdown();

	return {
		coldStartMs: stats(coldStart),
		firstDiagnosticMs: stats(firstDiagnostic.filter(Number.isFinite)),
		hoverMs: stats(hover),
		completionMs: stats(completion),
		peakRssMiB: Math.round(peakRss),
	};
}

function row(label, official, rsvelte, pick) {
	const a = pick(official);
	const b = pick(rsvelte);
	const ratio = a && b ? (a / b).toFixed(2) + '×' : '—';
	return `| ${label} | ${fmt(a)} | ${fmt(b)} | ${ratio} |`;
}

function fmt(value) {
	if (value == null || Number.isNaN(value)) return '—';
	return value >= 100 ? value.toFixed(0) : value.toFixed(1);
}

async function main() {
	const oracleBin = path.join(ORACLE_DIR, 'node_modules/svelte-language-server/bin/server.js');
	if (!fs.existsSync(oracleBin)) {
		die('LSP oracle not installed; run `npm --prefix scripts/compat-corpus/lsp-oracle install --no-package-lock`');
	}
	const rsvelteBin = ['release', 'debug']
		.map((p) => path.join(ROOT, 'target', p, 'rsvelte-language-server'))
		.find((p) => fs.existsSync(p));
	if (!rsvelteBin) die('rsvelte-language-server not built');

	// Both servers run against one copy, sequentially, so neither pays for the
	// other's page cache misses.
	const workspace = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'rsvelte-lsp-bench-')));
	fs.cpSync(PROJECT, workspace, {
		recursive: true,
		filter: (src) => !src.split(path.sep).includes('node_modules'),
	});
	fs.symlinkSync(path.join(ORACLE_DIR, 'node_modules'), path.join(workspace, 'node_modules'), 'dir');
	const files = components(workspace, 25);
	if (files.length === 0) die(`no .svelte components under ${PROJECT}`);

	console.log(`[lsp-bench] project: ${path.relative(ROOT, PROJECT)} (${files.length} components)`);
	const results = {};
	for (const side of ['official', 'rsvelte']) {
		results[side] = await measure(side, { workspace, oracleBin, rsvelteBin, files });
		console.log(`[lsp-bench] ${side} done`);
	}
	fs.rmSync(workspace, { recursive: true, force: true });

	const { official, rsvelte } = results;
	console.log('');
	console.log('| measurement | official (ms) | rsvelte (ms) | speedup |');
	console.log('|---|---|---|---|');
	console.log(row('cold start p50', official, rsvelte, (r) => r.coldStartMs.p50));
	console.log(row('first diagnostic p50', official, rsvelte, (r) => r.firstDiagnosticMs.p50));
	console.log(row('hover p50', official, rsvelte, (r) => r.hoverMs.p50));
	console.log(row('hover p90', official, rsvelte, (r) => r.hoverMs.p90));
	console.log(row('hover p99', official, rsvelte, (r) => r.hoverMs.p99));
	console.log(row('completion p50', official, rsvelte, (r) => r.completionMs.p50));
	console.log(row('completion p90', official, rsvelte, (r) => r.completionMs.p90));
	console.log(row('completion p99', official, rsvelte, (r) => r.completionMs.p99));
	console.log(
		`| peak RSS (MiB, process tree) | ${official.peakRssMiB} | ${rsvelte.peakRssMiB} | ${(official.peakRssMiB / Math.max(1, rsvelte.peakRssMiB)).toFixed(2)}× |`
	);

	if (JSON_OUT) {
		fs.writeFileSync(
			JSON_OUT,
			JSON.stringify({ project: path.relative(ROOT, PROJECT), runs: RUNS, results }, null, '\t') + '\n'
		);
		console.log(`\n[lsp-bench] wrote ${JSON_OUT}`);
	}
}

main().catch((err) => {
	console.error(err);
	process.exit(2);
});
