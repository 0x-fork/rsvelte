#!/usr/bin/env node
/**
 * The population the LSP parity gate compares, and the request stream it runs
 * over each unit.
 *
 * A PROJECT is a workspace root: one pair of servers is initialized against it
 * and every file under it is compared through that pair. A UNIT is one file in
 * one project; its id is `<project>/<relative path>` and that id is what the
 * ratchet keys on.
 */

import fs from 'node:fs';
import path from 'node:path';

/** Where the upstream suites live inside `submodules/language-tools`. */
const UPSTREAM = 'submodules/language-tools/packages/language-server/test/plugins/typescript';

/** Corpus repositories, deliberately the same four `corpus-sources.json` pins for real-world code. */
export const CORPUS_REPOS = [
	{ name: 'bits-ui', dir: 'submodules/bits-ui', src: 'packages/bits-ui/src' },
	{ name: 'flowbite-svelte', dir: 'submodules/flowbite-svelte', src: 'src/lib' },
	{ name: 'melt-ui', dir: 'submodules/melt-ui', src: 'packages/melt/src' },
	{ name: 'shadcn-svelte', dir: 'submodules/shadcn-svelte', src: 'docs/src/lib' },
];

/**
 * Whole-document requests. Every unit runs all of them; a server that does not
 * implement one answers `MethodNotFound`, which is itself a divergence class.
 */
export const DOCUMENT_METHODS = [
	'textDocument/documentSymbol',
	'textDocument/foldingRange',
	'textDocument/documentColor',
	'textDocument/codeLens',
	'textDocument/formatting',
	'textDocument/semanticTokens/full',
	'textDocument/inlayHint',
];

/** Requests that need a position; run at each sampled identifier. */
export const POSITION_METHODS = [
	'textDocument/hover',
	'textDocument/completion',
	'textDocument/definition',
	'textDocument/typeDefinition',
	'textDocument/documentHighlight',
	'textDocument/selectionRange',
];

/** Reserved words are never interesting subjects for hover/definition. */
const KEYWORDS = new Set(
	`await break case catch class const continue debugger default delete do else export extends finally for
	 function if import in instanceof let new return static super switch this throw try typeof var void while
	 with yield as from of true false null undefined string number boolean any unknown never type interface`.split(
		/\s+/
	)
);

/**
 * Deterministically sample identifier positions from a source text. Positions
 * are spread evenly through the identifier list rather than taken from the head,
 * so a file's markup is sampled as well as its `<script>`.
 */
export function samplePositions(text, limit) {
	const lines = text.split('\n');
	const found = [];
	for (let line = 0; line < lines.length; line++) {
		for (const m of lines[line].matchAll(/[A-Za-z_$][A-Za-z0-9_$]*/g)) {
			if (KEYWORDS.has(m[0]) || m[0].length < 2) continue;
			// Aim at the middle of the identifier: both servers resolve a position
			// inside a token, and the edges are ambiguous between two tokens.
			found.push({ line, character: m.index + Math.floor(m[0].length / 2) });
		}
	}
	if (limit === Infinity || found.length <= limit) return found;
	const step = found.length / limit;
	return Array.from({ length: limit }, (_, i) => found[Math.floor(i * step)]);
}

export function languageId(file) {
	if (file.endsWith('.svelte')) return 'svelte';
	if (file.endsWith('.ts')) return 'typescript';
	if (file.endsWith('.tsx')) return 'typescriptreact';
	if (file.endsWith('.js')) return 'javascript';
	return 'plaintext';
}

function listFiles(root, { extensions, limit = Infinity, skipDirs = new Set() }) {
	const out = [];
	const walk = (dir) => {
		let entries;
		try {
			entries = fs.readdirSync(dir, { withFileTypes: true });
		} catch {
			return;
		}
		for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
			if (entry.name.startsWith('.') || skipDirs.has(entry.name)) continue;
			const full = path.join(dir, entry.name);
			if (entry.isDirectory()) walk(full);
			else if (extensions.some((e) => entry.name.endsWith(e))) out.push(full);
		}
	};
	walk(root);
	out.sort();
	if (out.length <= limit) return out;
	// Evenly spaced, not the first N: a repository's first directory is not a
	// sample of the repository.
	const step = out.length / limit;
	return Array.from({ length: limit }, (_, i) => out[Math.floor(i * step)]);
}

/**
 * Build the project list. Every project is `{ name, root, files }` with `files`
 * relative to `root`; `root` is copied into both server workspaces verbatim.
 */
export function projects(ROOT, { corpusLimit }) {
	const list = [];
	const fixtures = path.join(ROOT, 'compatibility/lsp-fixtures');
	if (fs.existsSync(fixtures)) {
		for (const name of fs.readdirSync(fixtures).sort()) {
			const root = path.join(fixtures, name, 'project');
			if (!fs.existsSync(root)) continue;
			list.push({
				name: `fixtures/${name}`,
				root,
				files: listFiles(root, { extensions: ['.svelte', '.ts', '.js'] }).map((f) =>
					path.relative(root, f)
				),
			});
		}
	}

	const upstream = path.join(ROOT, UPSTREAM);
	for (const [name, rel] of [
		['folding-range', 'features/folding-range/fixtures'],
		['inlay-hints', 'features/inlayHints/fixtures'],
		['diagnostics', 'features/diagnostics/fixtures'],
		['testfiles', 'testfiles'],
		// The `svelte` plugin suite's own fixtures — TS-independent, and the only
		// upstream directory whose subject is the Svelte plugin rather than the
		// TypeScript one.
		['svelte-plugin', '../svelte/testfiles'],
	]) {
		const root = path.join(upstream, rel);
		if (!fs.existsSync(root)) continue;
		list.push({
			name: `upstream/${name}`,
			root,
			files: listFiles(root, { extensions: ['.svelte', '.ts'] }).map((f) => path.relative(root, f)),
		});
	}

	for (const repo of CORPUS_REPOS) {
		const base = path.join(ROOT, repo.dir);
		if (!fs.existsSync(base)) continue;
		const src = fs.existsSync(path.join(base, repo.src)) ? path.join(base, repo.src) : base;
		const files = listFiles(src, {
			extensions: ['.svelte'],
			limit: corpusLimit,
			skipDirs: new Set(['node_modules', 'dist', 'build']),
		});
		if (files.length === 0) continue;
		list.push({
			name: `corpus/${repo.name}`,
			root: src,
			files: files.map((f) => path.relative(src, f)),
		});
	}

	return list;
}

/** The upstream snapshot for a fixture directory, when the suite records one. */
export function upstreamSnapshot(projectRoot, file) {
	const dir = path.join(projectRoot, path.dirname(file));
	for (const name of ['expectedv2.json', 'expected.json']) {
		const p = path.join(dir, name);
		if (fs.existsSync(p)) {
			try {
				return JSON.parse(fs.readFileSync(p, 'utf8'));
			} catch {
				return undefined;
			}
		}
	}
	return undefined;
}
