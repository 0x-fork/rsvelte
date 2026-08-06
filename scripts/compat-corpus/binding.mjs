/**
 * Provenance for the staged NAPI binding.
 *
 * `.corpus-cache/rsvelte.node` is a copied build artifact. Nothing in the file
 * records which source it was built from, it is per-worktree, and it survives
 * every checkout, rebase and branch switch — so a measurement can be attributed
 * to a commit that never produced it.
 *
 * mtime cannot answer this. A binding built before a fix and copied after it has
 * a newer mtime than the fix it lacks, which is the shape that shipped a fixed
 * crash as a live bug: staged 02:44, missing a fix committed at 16:26 the
 * previous day. mtime older than the base is sound evidence of staleness; mtime
 * newer is evidence of nothing.
 *
 * So record the commit at stage time and check it at use time. Absent a stamp
 * the answer is "unknown", which is treated as unusable for writing a ratchet —
 * a ratchet entry is a durable claim that a divergence exists in a given tree.
 *
 * The stamp is `stagedAtCommit`, never `builtFromCommit`: it records what was
 * checked out when the library was copied, which is a lower bound on its age and
 * not evidence of what compiled it. #2482 upgrades this to a real attestation by
 * having the build embed its own commit; until then the field name is the honest
 * one, because the name is what a reader three weeks from now will trust.
 */

import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

export const BINDING_REL = '.corpus-cache/rsvelte.node';
const STAMP_REL = '.corpus-cache/rsvelte.node.provenance';

const git = (root, args) => {
	try {
		return execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
	} catch {
		return null;
	}
};

/**
 * Copy the freshly built library into place and stamp it in the same step.
 * Stamping without copying would attest whatever happened to be there, so the
 * stamp would manufacture the confidence it exists to justify.
 */
export function stageBinding(root) {
	const candidates = ['librsvelte_napi.dylib', 'librsvelte_napi.so'].map((n) =>
		path.join(root, 'target/release', n)
	);
	const built = candidates.find((p) => fs.existsSync(p));
	if (!built) {
		throw new Error(
			`no built library under target/release — run \`cargo build --release -p rsvelte_napi --lib\` first\n  looked for: ${candidates.map((p) => path.relative(root, p)).join(', ')}`
		);
	}
	// mtime cannot show a binary is current, but it can show one is stale: a
	// library older than the newest crates/ commit cannot contain it.
	const lastCratesCommit = git(root, ['log', '-1', '--format=%ct', '--', 'crates']);
	if (lastCratesCommit && fs.statSync(built).mtimeMs / 1000 < Number(lastCratesCommit)) {
		throw new Error(
			`${path.relative(root, built)} predates the newest commit touching crates/ — rebuild before staging, or the stamp would certify a binary that cannot contain it`
		);
	}
	fs.mkdirSync(path.join(root, '.corpus-cache'), { recursive: true });
	const dest = path.join(root, BINDING_REL);
	// Overwriting a loaded dylib in place leaves macOS holding a stale signature
	// for the path and it SIGKILLs anything that loads it, so land a new inode.
	const tmp = `${dest}.staging`;
	fs.copyFileSync(built, tmp);
	fs.renameSync(tmp, dest);
	const built_from = assertBindingLoads(root, dest);
	if (built_from?.commit && built_from.commit !== 'unknown') {
		const head = git(root, ['rev-parse', 'HEAD']);
		if (head && built_from.commit !== head) {
			fs.rmSync(dest, { force: true });
			throw new Error(
				`the built library reports commit ${built_from.commit.slice(0, 8)} but HEAD is ${head.slice(0, 8)} — ` +
					`it was compiled from a different tree, so staging it here would attest a commit that never produced it; ` +
					`removed it and wrote no stamp (rebuild at this HEAD)`
			);
		}
	}
	return writeBindingProvenance(root, built_from);
}

/**
 * Stamping a binding that cannot load is the failure this whole module exists to
 * prevent, one level down: the attestation would outlive the thing it describes.
 *
 * @returns the binary's self-reported build info, or null if it predates the export
 */
function assertBindingLoads(root, dest) {
	const probe = path.join(root, 'scripts/compat-corpus/binding-load-probe.mjs');
	let stdout;
	try {
		stdout = execFileSync(process.execPath, [probe, dest], { stdio: ['ignore', 'pipe', 'pipe'] });
	} catch (e) {
		fs.rmSync(dest, { force: true });
		const signal = e?.signal ? ` (${e.signal})` : '';
		throw new Error(
			`the staged binding does not load${signal} — removed it rather than stamping it; nothing was written to ${STAMP_REL}`
		);
	}
	try {
		return JSON.parse(String(stdout));
	} catch {
		return null;
	}
}

export function writeBindingProvenance(root, builtFrom = null) {
	const stagedAtCommit = git(root, ['rev-parse', 'HEAD']);
	if (!stagedAtCommit) throw new Error('cannot resolve HEAD to stamp the binding');
	// A dirty crates/ tree means the binding may contain uncommitted work, which
	// no commit id describes.
	const dirty = (git(root, ['status', '--porcelain', '--', 'crates']) ?? '') !== '';
	// builtFromCommit is what the binary reports about itself and is the field
	// worth trusting; stagedAtCommit only records what was checked out when the
	// file was copied, and is kept for bindings predating the export.
	const stamp = {
		builtFromCommit: builtFrom?.commit ?? null,
		builtDirty: builtFrom?.dirty ?? null,
		stagedAtCommit,
		dirty,
		stagedAt: new Date().toISOString(),
	};
	fs.writeFileSync(path.join(root, STAMP_REL), JSON.stringify(stamp, null, '\t') + '\n');
	return stamp;
}

/**
 * @returns {{state: 'ok'|'unknown'|'stale'|'dirty'|'foreign', detail: string}}
 */
export function bindingProvenance(root) {
	const stampPath = path.join(root, STAMP_REL);
	if (!fs.existsSync(stampPath)) {
		return {
			state: 'unknown',
			detail: `no ${STAMP_REL} — the binding's source commit was never recorded, so nothing it measures can be attributed to a tree (mtime cannot substitute: a binding built before a fix and copied after it looks newer than the fix it lacks)`,
		};
	}
	let stamp;
	try {
		stamp = JSON.parse(fs.readFileSync(stampPath, 'utf8'));
	} catch {
		return { state: 'unknown', detail: `${STAMP_REL} is unreadable` };
	}
	// Prefer what the binary reported about itself; fall back to the staging
	// observation only for libraries built before the export existed.
	const attested = typeof stamp.builtFromCommit === 'string' && stamp.builtFromCommit !== 'unknown';
	const at = attested ? stamp.builtFromCommit : stamp.stagedAtCommit;
	const how = attested ? 'built from' : 'staged at (inferred; binary reported nothing)';
	if (typeof at !== 'string' || at === '') {
		return { state: 'unknown', detail: `${STAMP_REL} records no commit` };
	}
	if (attested && stamp.builtDirty) {
		return {
			state: 'dirty',
			detail: `built from ${at.slice(0, 8)} with uncommitted changes under crates/`,
		};
	}
	if (!attested && stamp.dirty) {
		return { state: 'dirty', detail: `${how} ${at.slice(0, 8)} with uncommitted changes under crates/` };
	}
	const known = git(root, ['rev-parse', '--verify', `${at}^{commit}`]);
	if (!known) {
		return { state: 'foreign', detail: `${how} ${at.slice(0, 8)}, which is not a commit in this repository` };
	}
	const behind = git(root, ['rev-list', '--count', `${at}..HEAD`, '--', 'crates']);
	if (behind && behind !== '0') {
		return {
			state: 'stale',
			detail: `${how} ${at.slice(0, 8)}, which is ${behind} commit(s) touching crates/ behind HEAD`,
		};
	}
	return { state: 'ok', detail: `${how} ${at.slice(0, 8)}` };
}

/** A reason string when the binding cannot back a durable claim, else null. */
export function unattributedBindingReason(root) {
	const { state, detail } = bindingProvenance(root);
	if (state === 'ok') return null;
	return `the NAPI binding cannot be attributed: ${detail}; restage with \`node scripts/compat-corpus/binding.mjs --stage\``;
}

if (process.argv[1] && process.argv[1].endsWith('binding.mjs') && process.argv.includes('--stage')) {
	const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../..');
	try {
		const stamp = stageBinding(root);
		console.log(
			`[binding] staged at ${stamp.stagedAtCommit.slice(0, 8)}${stamp.dirty ? ' (DIRTY crates/)' : ''}`
		);
	} catch (e) {
		console.error(`[binding] ${e.message}`);
		process.exit(2);
	}
}
