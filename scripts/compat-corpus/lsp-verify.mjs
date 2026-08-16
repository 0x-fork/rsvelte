#!/usr/bin/env node
/**
 * LSP response-parity verifier — the differential harness for
 * `rsvelte-language-server` against the real `svelte-language-server`.
 *
 * Upstream ships **no** end-to-end protocol test: every `language-server` test
 * constructs a plugin class in-process. So both sides are driven here over
 * stdio, with the *same* `initialize` params, the *same* client capabilities
 * and the *same* request stream, and their responses are diffed.
 *
 *   1. Materialise each project twice — one tree per server, so neither can see
 *      files the other emitted — and symlink the pinned oracle's `node_modules`
 *      into both, so both resolve one dependency tree.
 *   2. Initialize a server pair against the project root.
 *   3. For every file: `didOpen` on both, run the request stream, wait for
 *      pushed diagnostics, `didClose`.
 *   4. Reduce each response pair to a VERDICT (`lsp/normalize.mjs`) and ratchet.
 *
 * WHAT THE RATCHET STORES is `<unit>|<method>|<verdict>` — a class, not a
 * payload. Keying on the payload would churn on every TypeScript wording change;
 * keying on `<unit>|<method>` alone would let a divergence that changes kind
 * reuse an existing entry, which is the failure mode #2521 recorded for the
 * shape matrix. The verdict names the class: `only-official` / `only-rsvelte`
 * (one side answered nothing), `count`, `differs:<fields>`, `error-*`.
 *
 * Positions are SAMPLED, not exhaustive (`--positions`), and so are the files of
 * the larger projects (`--max-units`, `--corpus-limit`). Those three numbers are
 * part of the gate's definition, not a convenience: `--update` refuses to run
 * when any of them is overridden, because a baseline written from a subset
 * deletes every entry the subset did not measure.
 *
 * Usage:
 *   node scripts/compat-corpus/lsp-verify.mjs                 # verify (CI gate)
 *   node scripts/compat-corpus/lsp-verify.mjs --update        # rewrite the ratchet
 *   node scripts/compat-corpus/lsp-verify.mjs --project a,b   # restrict projects
 *   node scripts/compat-corpus/lsp-verify.mjs --show N        # print up to N new entries
 *   node scripts/compat-corpus/lsp-verify.mjs --keep          # keep the temp workspaces
 *   node scripts/compat-corpus/lsp-verify.mjs --trace         # echo both servers' stderr
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { LspClient, LspTimeout, clientCapabilities } from './lsp/client.mjs';
import { makeUriNormalizer, project as projectResponse, stable, verdict } from './lsp/normalize.mjs';
import {
	DOCUMENT_METHODS,
	POSITION_METHODS,
	languageId,
	projects,
	samplePositions,
	upstreamSnapshot,
} from './lsp/population.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '../..');
const ORACLE_DIR = path.join(__dirname, 'lsp-oracle');
const KNOWN = path.join(ROOT, 'compatibility/lsp-known-failures.json');
const REPORT = path.join(ROOT, 'compatibility/lsp-report.json');

// The three numbers that define the population. Changing one changes what the
// ratchet means, so they live here rather than in a flag default.
const DEFAULT_POSITIONS = 4;
const DEFAULT_MAX_UNITS = 40;
const DEFAULT_CORPUS_LIMIT = 25;
// `--update` deletes every id it did not measure. A run against un-checked-out
// submodules would otherwise silently empty the ratchet and report a clean gate.
const MIN_UNITS = 150;

const args = process.argv.slice(2);
const UPDATE = args.includes('--update');
const KEEP = args.includes('--keep');
const TRACE = args.includes('--trace');
const SHOW = numArg('--show', 50);
const POSITIONS = numArg('--positions', DEFAULT_POSITIONS);
const MAX_UNITS = numArg('--max-units', DEFAULT_MAX_UNITS);
const CORPUS_LIMIT = numArg('--corpus-limit', DEFAULT_CORPUS_LIMIT);
const REQUEST_TIMEOUT = numArg('--request-timeout', 30_000);
const WARMUP_TIMEOUT = numArg('--warmup-timeout', 180_000);
const ONLY = args.includes('--project')
	? new Set((args[args.indexOf('--project') + 1] || '').split(',').filter(Boolean))
	: null;

function numArg(flag, fallback) {
	if (!args.includes(flag)) return fallback;
	const value = Number(args[args.indexOf(flag) + 1]);
	if (!Number.isFinite(value) || value <= 0) fail(`${flag} needs a positive number`);
	return value;
}

function fail(msg) {
	console.error(`[lsp-verify] ${msg}`);
	process.exit(2);
}

if (UPDATE && ONLY) fail('--update cannot be combined with --project');
if (
	UPDATE &&
	(POSITIONS !== DEFAULT_POSITIONS ||
		MAX_UNITS !== DEFAULT_MAX_UNITS ||
		CORPUS_LIMIT !== DEFAULT_CORPUS_LIMIT)
) {
	fail('--update cannot be combined with --positions/--max-units/--corpus-limit');
}

function readJson(file, what) {
	try {
		return JSON.parse(fs.readFileSync(file, 'utf8'));
	} catch (err) {
		return fail(`${what} is not readable JSON (${path.relative(ROOT, file)}): ${err.message}`);
	}
}

function rsvelteBinary() {
	for (const profile of ['release', 'debug']) {
		const p = path.join(ROOT, 'target', profile, 'rsvelte-language-server');
		if (fs.existsSync(p)) return p;
	}
	return fail(
		'rsvelte-language-server not found; run `cargo build --release -p rsvelte_language_server`'
	);
}

function oracleServer() {
	const nm = path.join(ORACLE_DIR, 'node_modules');
	const bin = path.join(nm, 'svelte-language-server/bin/server.js');
	if (!fs.existsSync(bin)) {
		return fail(
			'LSP oracle not installed; run `npm --prefix scripts/compat-corpus/lsp-oracle install --no-package-lock`'
		);
	}
	return { nodeModules: nm, bin };
}

function materialize(source, dest, nodeModules) {
	fs.rmSync(dest, { recursive: true, force: true });
	fs.mkdirSync(dest, { recursive: true });
	fs.cpSync(source, dest, {
		recursive: true,
		filter: (src) => !src.split(path.sep).includes('node_modules'),
	});
	fs.symlinkSync(nodeModules, path.join(dest, 'node_modules'), 'dir');
}

function fileUri(p) {
	return 'file://' + p.split(path.sep).join('/');
}

async function startPair({ oracleBin, rsvelteBin, oracleDir, actualDir }) {
	const capabilities = clientCapabilities();
	const spawnOne = (label, command, cmdArgs, cwd) =>
		new LspClient({
			label,
			command,
			args: cmdArgs,
			cwd,
			timeoutMs: REQUEST_TIMEOUT,
			trace: TRACE,
		});

	const official = spawnOne('official', process.execPath, [oracleBin, '--stdio'], oracleDir);
	const rsvelte = spawnOne('rsvelte', rsvelteBin, ['--stdio'], actualDir);

	await Promise.all(
		[
			[official, oracleDir],
			[rsvelte, actualDir],
		].map(async ([client, dir]) => {
			await client.request(
				'initialize',
				{
					processId: process.pid,
					clientInfo: { name: 'rsvelte-lsp-parity', version: '1' },
					rootUri: fileUri(dir),
					rootPath: dir,
					workspaceFolders: [{ uri: fileUri(dir), name: path.basename(dir) }],
					capabilities,
					initializationOptions: { isTrusted: true },
				},
				{ timeoutMs: 120_000 }
			);
			client.notify('initialized', {});
		})
	);
	return { official, rsvelte };
}

/** Run one method against one server, flattening transport failures into a class. */
async function ask(client, method, params) {
	try {
		return { value: await client.request(method, params) };
	} catch (err) {
		if (err instanceof LspTimeout) return { error: 'timeout' };
		if (err.name === 'LspError') return { error: String(err.code) };
		return { error: 'crash' };
	}
}

function documentParams(method, uri, text, position) {
	const doc = { textDocument: { uri } };
	switch (method) {
		case 'textDocument/formatting':
			return { ...doc, options: { tabSize: 2, insertSpaces: false } };
		case 'textDocument/inlayHint': {
			const lines = text.split('\n');
			return {
				...doc,
				range: {
					start: { line: 0, character: 0 },
					end: { line: lines.length - 1, character: lines[lines.length - 1].length },
				},
			};
		}
		case 'textDocument/completion':
			return { ...doc, position, context: { triggerKind: 1 } };
		case 'textDocument/selectionRange':
			return { ...doc, positions: [position] };
		case 'textDocument/codeAction':
			return { ...doc, range: { start: position, end: position }, context: { diagnostics: [] } };
		default:
			return position ? { ...doc, position } : doc;
	}
}

/**
 * Open every file of a project on both servers and wait until each has answered
 * for them. A TypeScript-backed response depends on whether the program has
 * finished loading, so comparing a file the moment it is opened records a race
 * rather than a divergence — the baseline would then differ between machines.
 */
async function warmup(pair, units, timeoutMs) {
	for (const unit of units) {
		for (const [side, client] of Object.entries(pair)) {
			client.notify('textDocument/didOpen', {
				textDocument: {
					uri: unit.uris[side],
					languageId: unit.language,
					version: 1,
					text: unit.text,
				},
			});
		}
	}
	await Promise.all(
		Object.entries(pair).map(([side, client]) =>
			client.waitFor(() => units.every((u) => client.diagnostics.has(u.uris[side])), {
				timeoutMs,
				intervalMs: 50,
			})
		)
	);
	// One request per server that only answers once the program is up; its
	// result is discarded, it exists to drain the load queue.
	await Promise.all(
		Object.entries(pair).map(([side, client]) =>
			ask(client, 'textDocument/documentSymbol', {
				textDocument: { uri: units[0].uris[side] },
			})
		)
	);
}

async function compareUnit({ pair, unit, normalizeUri, divergences, detail, tally }) {
	const { id: unitId, text, uris } = unit;

	const record = (method, official, rsvelte) => {
		tally.compared++;
		const v = verdict(
			{ ...official, value: official.error ? null : projectResponse(method, official.value, normalizeUri) },
			{ ...rsvelte, value: rsvelte.error ? null : projectResponse(method, rsvelte.value, normalizeUri) }
		);
		if (!v) {
			tally.agreed++;
			return;
		}
		const key = `${unitId}|${method.replace('textDocument/', '')}|${v}`;
		if (divergences.has(key)) return;
		divergences.add(key);
		if (detail.length < 400) {
			detail.push({
				key,
				official: truncate(official.error ?? projectResponse(method, official.value, normalizeUri)),
				rsvelte: truncate(rsvelte.error ?? projectResponse(method, rsvelte.value, normalizeUri)),
			});
		}
	};

	for (const method of DOCUMENT_METHODS) {
		const params = { official: null, rsvelte: null };
		for (const side of ['official', 'rsvelte']) {
			params[side] = documentParams(method, uris[side], text, null);
		}
		const [a, b] = await Promise.all([
			ask(pair.official, method, params.official),
			ask(pair.rsvelte, method, params.rsvelte),
		]);
		record(method, a, b);
		// The oracle's positive control. Upstream records what its own folding
		// provider answers for these fixtures; if driving the official server
		// over stdio stopped reproducing them, every verdict in this run would
		// be measured against a server that is not behaving as upstream's tests
		// say it does — and nothing else here could tell.
		if (unit.snapshot && method === 'textDocument/foldingRange' && !a.error) {
			tally.calibrated++;
			const want = stable(projectResponse(method, unit.snapshot, normalizeUri));
			if (stable(projectResponse(method, a.value, normalizeUri)) === want) tally.reproduced++;
		}
	}

	for (const position of samplePositions(text, POSITIONS)) {
		for (const method of POSITION_METHODS) {
			const [a, b] = await Promise.all([
				ask(pair.official, method, documentParams(method, uris.official, text, position)),
				ask(pair.rsvelte, method, documentParams(method, uris.rsvelte, text, position)),
			]);
			record(method, a, b);
		}
	}

	// Diagnostics are pushed, not requested, and the wait for them already
	// happened in `warmup`; what is read here is the latest published state.
	// Compared per `source`, not as one list. rsvelte's server also publishes
	// its native linter's findings (`source: "rsvelte"`), which official has no
	// counterpart for; folded into one key they would mask every TypeScript or
	// compiler diagnostic divergence in the same file behind one entry.
	const bySource = {
		official: groupBySource(pair.official.diagnostics.get(uris.official)),
		rsvelte: groupBySource(pair.rsvelte.diagnostics.get(uris.rsvelte)),
	};
	for (const source of new Set([...bySource.official.keys(), ...bySource.rsvelte.keys()])) {
		record(
			`textDocument/publishDiagnostics[${source}]`,
			{ value: bySource.official.get(source) ?? null },
			{ value: bySource.rsvelte.get(source) ?? null }
		);
	}
}

function groupBySource(diagnostics) {
	const groups = new Map();
	for (const d of diagnostics ?? []) {
		const source = d.source ?? 'none';
		if (!groups.has(source)) groups.set(source, []);
		groups.get(source).push(d);
	}
	return groups;
}

function truncate(value) {
	const text = typeof value === 'string' ? value : JSON.stringify(value ?? null);
	return text.length > 600 ? text.slice(0, 600) + '…' : text;
}

async function main() {
	const rsvelteBin = rsvelteBinary();
	const { nodeModules, bin: oracleBin } = oracleServer();
	const all = projects(ROOT, { corpusLimit: CORPUS_LIMIT });
	const selected = all.filter((p) => !ONLY || ONLY.has(p.name));
	if (selected.length === 0) fail('no projects to compare (are the submodules checked out?)');

	// `realpath`: on macOS `os.tmpdir()` is a symlink into `/private`, and the
	// two servers resolve it differently — one answers `file:///var/…`, the other
	// `file:///private/var/…`, which would read as a divergence of every location.
	const tmpRoot = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'rsvelte-lsp-parity-')));
	const divergences = new Set();
	const detail = [];
	const tallies = {};
	let unitCount = 0;

	for (const proj of selected) {
		const files = proj.files.slice(0, MAX_UNITS);
		if (files.length === 0) continue;
		const base = path.join(tmpRoot, proj.name.replace(/\//g, '__'));
		const oracleDir = path.join(base, 'oracle');
		const actualDir = path.join(base, 'actual');
		materialize(proj.root, oracleDir, nodeModules);
		materialize(proj.root, actualDir, nodeModules);
		const normalizeUri = makeUriNormalizer([oracleDir, actualDir, nodeModules, ROOT]);

		const units = files.map((relPath) => ({
			id: `${proj.name}/${relPath.split(path.sep).join('/')}`,
			text: fs.readFileSync(path.join(oracleDir, relPath), 'utf8'),
			language: languageId(relPath),
			uris: {
				official: fileUri(path.join(oracleDir, relPath)),
				rsvelte: fileUri(path.join(actualDir, relPath)),
			},
			// Only the folding-range suite records a snapshot the protocol can be
			// compared against; the others' expectations are TypeScript-backed and
			// were produced with provider-level options no LSP request carries.
			snapshot:
				proj.name === 'upstream/folding-range' ? upstreamSnapshot(oracleDir, relPath) : undefined,
		}));

		const pair = await startPair({ oracleBin, rsvelteBin, oracleDir, actualDir });
		const started = Date.now();
		const tally = { compared: 0, agreed: 0, calibrated: 0, reproduced: 0 };
		try {
			await warmup(pair, units, WARMUP_TIMEOUT);
			for (const unit of units) {
				await compareUnit({ pair, unit, normalizeUri, divergences, detail, tally });
				unitCount++;
			}
		} finally {
			await Promise.all([pair.official.shutdown(), pair.rsvelte.shutdown()]);
		}
		tallies[proj.name] = tally;
		if (tally.calibrated > 0) {
			console.log(
				`[lsp-verify] ${proj.name}: oracle reproduces ${tally.reproduced}/${tally.calibrated} upstream snapshot(s)`
			);
		}
		console.log(
			`[lsp-verify] ${proj.name}: ${units.length} unit(s), ${tally.agreed}/${tally.compared} responses agree, in ${((Date.now() - started) / 1000).toFixed(1)}s`
		);
		// A pair that agrees on nothing is not a pair that diverges everywhere —
		// it is a pair where one server never came up. That state produces a
		// full ratchet of `only-official` entries and reads like a measurement.
		if (tally.agreed === 0) {
			fail(`${proj.name}: not one response agreed — the server pair did not come up`);
		}
	}

	if (!KEEP) fs.rmSync(tmpRoot, { recursive: true, force: true });
	else console.log(`[lsp-verify] temp workspaces kept at ${tmpRoot}`);

	const current = [...divergences].sort();
	fs.writeFileSync(
		REPORT,
		JSON.stringify(
			// `entries` is the whole current divergence set, not a sample: a CI
			// failure is only actionable if the run's own answer can be read off
			// the uploaded artifact, and `detail` is capped.
			{ units: unitCount, divergences: current.length, projects: tallies, entries: current, detail },
			null,
			'\t'
		) + '\n'
	);

	// A run that compared nothing produces an empty diff, which is
	// indistinguishable from full parity — assert the population instead.
	if (unitCount < MIN_UNITS && (UPDATE || !ONLY)) {
		fail(`only ${unitCount} unit(s) compared, expected at least ${MIN_UNITS}`);
	}

	// The oracle's positive control, aggregated. Measured when this landed:
	// 13 of 15, the two misses being one extra whole-`<script>` fold each, which
	// the HTML provider contributes and upstream's provider-level test does not
	// run. A drop below the floor means the official server is no longer being
	// driven the way its own tests say it behaves, which invalidates every
	// verdict in the run — so it is fatal rather than a ratchet entry.
	const calibrated = Object.values(tallies).reduce((a, t) => a + t.calibrated, 0);
	const reproduced = Object.values(tallies).reduce((a, t) => a + t.reproduced, 0);
	if (calibrated > 0) {
		console.log(`[lsp-verify] oracle calibration: ${reproduced}/${calibrated} upstream snapshots reproduced`);
		if (reproduced / calibrated < 0.8) {
			fail(`oracle reproduces only ${reproduced}/${calibrated} upstream snapshots — it is not being driven correctly`);
		}
	} else if (!ONLY) {
		fail('no upstream snapshot was compared — is submodules/language-tools checked out?');
	}

	const known = fs.existsSync(KNOWN) ? readJson(KNOWN, 'the ratchet') : [];
	const knownSet = new Set(known);
	const currentSet = new Set(current);
	const added = current.filter((d) => !knownSet.has(d));
	const removed = known.filter((d) => !currentSet.has(d));

	console.log(
		`[lsp-verify] ${unitCount} units, ${current.length} divergences, ${known.length} known (${added.length} new, ${removed.length} fixed)`
	);

	if (UPDATE) {
		fs.writeFileSync(KNOWN, JSON.stringify(current, null, '\t') + '\n');
		console.log(`[lsp-verify] wrote ${current.length} entries to ${path.relative(ROOT, KNOWN)}`);
		return;
	}

	if (added.length > 0) {
		console.error(`\n[lsp-verify] ❌ ${added.length} NEW divergence(s) from svelte-language-server:`);
		for (const d of added.slice(0, SHOW)) console.error('  ' + d);
		if (added.length > SHOW) console.error(`  … and ${added.length - SHOW} more`);
		console.error(`\n  (details in ${path.relative(ROOT, REPORT)})`);
	}
	// Staleness is fatal for the same reason it is in check-verify.mjs: a large
	// "already fixed" delta on a later PR reads as noise a regression can hide in.
	if (removed.length > 0) {
		console.error(`\n[lsp-verify] ❌ ${removed.length} ratchet entries no longer diverge — the ratchet is stale.`);
		for (const d of removed.slice(0, SHOW)) console.error('  ' + d);
		if (removed.length > SHOW) console.error(`  … and ${removed.length - SHOW} more`);
		console.error('\n  fix: node scripts/compat-corpus/lsp-verify.mjs --update');
	}
	if (added.length > 0 || removed.length > 0) process.exit(1);
	console.log('[lsp-verify] ✅ no new divergences');
}

main().catch((err) => {
	console.error(err);
	process.exit(2);
});
