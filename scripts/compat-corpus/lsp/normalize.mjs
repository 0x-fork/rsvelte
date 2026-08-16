#!/usr/bin/env node
/**
 * Turns a raw LSP response into the value the parity gate compares, and turns a
 * pair of those into a *verdict* — a coarse, stable class of divergence.
 *
 * The verdict, not the payload, is what the ratchet stores. A payload key would
 * churn on every wording change in a TypeScript message; a class key does not,
 * and it still fails when a divergence changes kind (a ratchet entry suppresses
 * everything its key cannot tell apart).
 */

/** Keys that are server-private by specification, or unstable by construction. */
const DROPPED_KEYS = new Set([
	// Opaque round-trip payload for `*/resolve`; the spec says clients must not read it.
	'data',
	// Both servers stamp their own name here.
	'source',
]);

/**
 * The exception to `data` being opaque: on a semantic-tokens response it IS the
 * answer. Dropping it there made every token stream compare equal to every
 * other — the gate reported zero semantic-token divergences over 227 files while
 * one server was returning tokens and the other an empty array.
 */
const TOKEN_KEYS = new Set(['source']);

/**
 * Rewrite absolute `file://` URIs (and bare absolute paths) that point inside a
 * materialised workspace to `file:///W/<rel>`, so the two trees compare equal.
 */
export function makeUriNormalizer(roots) {
	const prefixes = [];
	for (const root of roots.filter(Boolean)) {
		// Both spellings a server can emit for the same directory: the raw path,
		// and the percent-encoded form `vscode-uri` produces (a checkout path
		// holding `+` comes back as `%2B`, which no prefix match on the raw path
		// would ever see).
		for (const dir of new Set([root, percentEncodePath(root)])) {
			prefixes.push([pathToFileUrl(dir), dir]);
		}
	}
	prefixes.sort((a, b) => b[0].length - a[0].length);
	return (text) => {
		let out = text;
		for (const [uri, dir] of prefixes) {
			out = splitJoin(out, uri, 'file:///W');
			out = splitJoin(out, dir, '/W');
		}
		return out;
	};
}

function percentEncodePath(p) {
	return p
		.split('/')
		.map((segment) => encodeURIComponent(segment))
		.join('/');
}

function splitJoin(text, needle, replacement) {
	return text.includes(needle) ? text.split(needle).join(replacement) : text;
}

function pathToFileUrl(p) {
	// Matches what both servers emit: `file://` + POSIX path, no percent-encoding
	// of the separators. Only the prefix has to match, so this stays deliberately
	// simple rather than reimplementing `pathToFileURL`.
	return 'file://' + (p.startsWith('/') ? p : '/' + p.replace(/\\/g, '/'));
}

/**
 * Canonical form of a response: URIs rewritten, private keys dropped, unordered
 * arrays sorted. `method` selects the projection — comparing a completion list
 * item-for-item would make every run a diff of TypeScript's global scope.
 */
export function project(method, value, normalizeUri) {
	// `publishDiagnostics[<source>]` and friends carry the sub-population in the
	// key; the projection is the base method's.
	const base = method.replace(/\[[^\]]*\]$/, '');
	const cleaned = clean(value, normalizeUri, base.startsWith('textDocument/semanticTokens') ? TOKEN_KEYS : DROPPED_KEYS);
	switch (base) {
		case 'textDocument/completion': {
			const list = cleaned == null ? null : Array.isArray(cleaned) ? cleaned : cleaned.items;
			if (list == null) return null;
			const labels = list
				.map((item) => `${item.label} ${item.kind ?? ''} ${item.insertTextFormat ?? ''}`)
				.sort();
			return { isIncomplete: cleaned.isIncomplete ?? false, items: labels };
		}
		case 'textDocument/hover': {
			if (cleaned == null) return null;
			return { range: cleaned.range ?? null, contents: hoverText(cleaned.contents) };
		}
		case 'textDocument/definition':
		case 'textDocument/typeDefinition':
		case 'textDocument/implementation':
		case 'textDocument/references':
			return locations(cleaned);
		case 'textDocument/publishDiagnostics':
			return sortedBy(cleaned, (d) => `${pos(d.range)} ${d.severity} ${d.code} ${d.message}`);
		case 'textDocument/documentHighlight':
			return sortedBy(cleaned, (h) => `${pos(h.range)} ${h.kind ?? ''}`);
		case 'textDocument/foldingRange':
			return sortedBy(cleaned, (f) => `${f.startLine}:${f.startCharacter ?? ''}-${f.endLine}:${f.endCharacter ?? ''}:${f.kind ?? ''}`);
		case 'textDocument/documentColor':
			return sortedBy(cleaned, (c) => pos(c.range));
		case 'textDocument/codeAction':
			return sortedBy(cleaned, (a) => `${a.title} ${a.kind ?? ''}`);
		case 'textDocument/codeLens':
			return sortedBy(cleaned, (l) => `${pos(l.range)} ${l.command?.title ?? ''}`);
		case 'textDocument/inlayHint':
			return sortedBy(cleaned, (h) => `${h.position.line}:${h.position.character} ${hintLabel(h.label)}`);
		case 'textDocument/semanticTokens/full':
			// The relative-encoded `data` array is compared whole; a length-only
			// key would call two different tokenizations equal.
			return cleaned == null ? null : { data: cleaned.data ?? [] };
		default:
			return cleaned;
	}
}

function locations(value) {
	if (value == null) return null;
	const list = Array.isArray(value) ? value : [value];
	return list
		.map((l) => {
			const uri = l.uri ?? l.targetUri;
			const range = l.range ?? l.targetSelectionRange ?? l.targetRange;
			return `${uri} ${pos(range)}`;
		})
		.sort();
}

function hoverText(contents) {
	if (contents == null) return null;
	if (typeof contents === 'string') return contents;
	if (Array.isArray(contents)) return contents.map(hoverText).join('\n---\n');
	if (typeof contents.value === 'string') return contents.value;
	return JSON.stringify(contents);
}

function hintLabel(label) {
	if (typeof label === 'string') return label;
	if (Array.isArray(label)) return label.map((p) => p.value).join('');
	return String(label);
}

function pos(range) {
	if (!range) return '';
	return `${range.start.line}:${range.start.character}-${range.end.line}:${range.end.character}`;
}

function sortedBy(value, key) {
	if (value == null) return null;
	if (!Array.isArray(value)) return value;
	return value.map((v) => [key(v), v]).sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)).map(([, v]) => v);
}

function clean(value, normalizeUri, dropped) {
	if (value == null) return null;
	if (typeof value === 'string') return normalizeUri(value);
	if (Array.isArray(value)) return value.map((v) => clean(v, normalizeUri, dropped));
	if (typeof value !== 'object') return value;
	const out = {};
	for (const key of Object.keys(value).sort()) {
		if (dropped.has(key)) continue;
		out[key] = clean(value[key], normalizeUri, dropped);
	}
	return out;
}

/** `null`, `[]`, `{items: []}` and an empty string all mean "the server had nothing". */
export function isEmpty(value) {
	if (value == null) return true;
	if (Array.isArray(value)) return value.length === 0;
	if (typeof value === 'object') {
		if (Array.isArray(value.items)) return value.items.length === 0;
		if (Array.isArray(value.data)) return value.data.length === 0;
		if ('contents' in value) return value.contents == null || value.contents === '';
		return Object.keys(value).length === 0;
	}
	return value === '';
}

/**
 * The comparison. Returns `null` when the two sides agree, otherwise a verdict
 * string that names the CLASS of the divergence.
 */
export function verdict(official, rsvelte) {
	if (official.error || rsvelte.error) {
		if (official.error && rsvelte.error) {
			return official.error === rsvelte.error ? null : `error-both:${official.error}/${rsvelte.error}`;
		}
		return official.error ? `error-official:${official.error}` : `error-rsvelte:${rsvelte.error}`;
	}
	const a = official.value;
	const b = rsvelte.value;
	const emptyA = isEmpty(a);
	const emptyB = isEmpty(b);
	if (emptyA && emptyB) return null;
	if (emptyA) return 'only-rsvelte';
	if (emptyB) return 'only-official';
	if (stable(a) === stable(b)) return null;
	const countA = countOf(a);
	const countB = countOf(b);
	if (countA !== null && countB !== null && countA !== countB) return 'count';
	const fields = differingFields(a, b);
	return fields.length ? `differs:${fields.join(',')}` : 'differs';
}

function countOf(value) {
	if (Array.isArray(value)) return value.length;
	if (value && Array.isArray(value.items)) return value.items.length;
	if (value && Array.isArray(value.data)) return value.data.length;
	return null;
}

/**
 * The union of object keys whose values differ, walked one array level deep and
 * capped — a verdict has to stay short enough to read in a ratchet file.
 */
function differingFields(a, b) {
	const found = new Set();
	walk(a, b, found, 0);
	return [...found].sort().slice(0, 3);
}

function walk(a, b, found, depth) {
	if (found.size >= 6 || depth > 3) return;
	if (Array.isArray(a) && Array.isArray(b)) {
		for (let i = 0; i < Math.min(a.length, b.length); i++) walk(a[i], b[i], found, depth + 1);
		return;
	}
	if (a && b && typeof a === 'object' && typeof b === 'object') {
		for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
			if (stable(a[key]) !== stable(b[key])) {
				found.add(key);
				walk(a[key], b[key], found, depth + 1);
			}
		}
		return;
	}
}

export function stable(value) {
	return JSON.stringify(value ?? null);
}
