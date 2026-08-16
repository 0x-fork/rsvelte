#!/usr/bin/env node
/**
 * The LSP parity gate's comparison key (`scripts/compat-corpus/lsp/normalize.mjs`).
 *
 * It exists because that module already shipped one silent-agreement bug: `data`
 * was dropped everywhere as the opaque resolve-request payload, which also erased a
 * semantic-tokens response's entire answer — the gate then reported **zero**
 * semantic-token divergences over 227 files while one server was returning
 * tokens and the other an empty array. A projection that throws away the field
 * under comparison produces a clean green, so every case below is written as
 * "these two must NOT compare equal".
 *
 * Runs without submodules, without a build and without either server.
 */

import assert from 'node:assert/strict';
import { isEmpty, makeUriNormalizer, project, verdict } from '../compat-corpus/lsp/normalize.mjs';

const keep = (s) => s;
let failures = 0;

function check(name, fn) {
	try {
		fn();
		console.log(`  ok   ${name}`);
	} catch (err) {
		failures++;
		console.error(`  FAIL ${name}: ${err.message}`);
	}
}

const compare = (method, official, rsvelte) =>
	verdict({ value: project(method, official, keep) }, { value: project(method, rsvelte, keep) });

console.log('lsp/normalize.mjs');

check('semantic tokens: a token stream does not equal an empty one', () => {
	assert.equal(
		compare('textDocument/semanticTokens/full', { resultId: '1', data: [4, 5, 5, 7, 33] }, { data: [] }),
		'only-official'
	);
});

check('semantic tokens: two different tokenizations of the same length differ', () => {
	assert.equal(
		compare('textDocument/semanticTokens/full', { data: [0, 1, 2, 3, 4] }, { data: [0, 1, 2, 3, 9] }),
		'differs:data'
	);
});

check('completion: `data` is dropped, the label set is not', () => {
	assert.equal(
		compare(
			'textDocument/completion',
			{ isIncomplete: false, items: [{ label: 'a', kind: 1, data: { file: 'x' } }] },
			{ isIncomplete: false, items: [{ label: 'a', kind: 1, data: { file: 'y' } }] }
		),
		null
	);
	assert.equal(
		compare(
			'textDocument/completion',
			{ isIncomplete: false, items: [{ label: 'a', kind: 1 }] },
			{ isIncomplete: false, items: [{ label: 'b', kind: 1 }] }
		),
		'differs:items'
	);
});

check('completion: item ORDER is deliberately not compared', () => {
	assert.equal(
		compare(
			'textDocument/completion',
			{ isIncomplete: false, items: [{ label: 'a' }, { label: 'b' }] },
			{ isIncomplete: false, items: [{ label: 'b' }, { label: 'a' }] }
		),
		null,
		'sorting the labels is the documented blind spot 27d; if this starts failing the doc is stale'
	);
});

check('hover: differing text is a divergence, not agreement', () => {
	const range = { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } };
	assert.equal(
		compare(
			'textDocument/hover',
			{ range, contents: { kind: 'markdown', value: 'a' } },
			{ range, contents: { kind: 'markdown', value: 'a\n' } }
		),
		'differs:contents'
	);
});

check('diagnostics: order does not matter, content does', () => {
	const one = { range: r(0), severity: 1, code: 1, message: 'x' };
	const two = { range: r(1), severity: 1, code: 2, message: 'y' };
	assert.equal(compare('textDocument/publishDiagnostics[ts]', [one, two], [two, one]), null);
	assert.equal(
		compare('textDocument/publishDiagnostics[ts]', [one], [{ ...one, code: 99 }]),
		'differs:code'
	);
});

check('a missing answer on one side is named by side', () => {
	assert.equal(compare('textDocument/foldingRange', [{ startLine: 0, endLine: 1 }], []), 'only-official');
	assert.equal(compare('textDocument/foldingRange', [], [{ startLine: 0, endLine: 1 }]), 'only-rsvelte');
	assert.equal(compare('textDocument/foldingRange', null, []), null);
});

check('a length difference is `count`, not a field list', () => {
	assert.equal(
		compare(
			'textDocument/foldingRange',
			[{ startLine: 0, endLine: 1 }, { startLine: 2, endLine: 3 }],
			[{ startLine: 0, endLine: 1 }]
		),
		'count'
	);
});

check('errors carry the side and the code', () => {
	assert.equal(verdict({ error: '-32601' }, { value: null }), 'error-official:-32601');
	assert.equal(verdict({ value: null }, { error: 'timeout' }), 'error-rsvelte:timeout');
	assert.equal(verdict({ error: '-32601' }, { error: '-32601' }), null);
	assert.equal(verdict({ error: '-32601' }, { error: 'timeout' }), 'error-both:-32601/timeout');
});

check('"no answer" and "an answer of zero things" are one state, on purpose', () => {
	// `null` vs `[]` is not a difference a user can observe, so the gate does
	// not spend a verdict on it.
	assert.equal(isEmpty(null), true);
	assert.equal(isEmpty([]), true);
	assert.equal(isEmpty({ data: [] }), true);
	assert.equal(isEmpty({ items: [] }), true);
	assert.equal(isEmpty({ data: [1] }), false);
});

check('workspace paths are normalized away, percent-encoded or not', () => {
	const n = makeUriNormalizer(['/tmp/a+b/oracle']);
	assert.equal(n('file:///tmp/a+b/oracle/src/x.ts'), 'file:///W/src/x.ts');
	assert.equal(n('file:///tmp/a%2Bb/oracle/src/x.ts'), 'file:///W/src/x.ts');
});

function r(line) {
	return { start: { line, character: 0 }, end: { line, character: 1 } };
}

if (failures > 0) {
	console.error(`\n${failures} check(s) failed`);
	process.exit(1);
}
console.log('\nall checks passed');
