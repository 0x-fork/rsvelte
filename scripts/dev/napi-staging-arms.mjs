#!/usr/bin/env node
/**
 * Why re-staging a NAPI binding can produce a library the kernel SIGKILLs on
 * load, and which staging procedure avoids it.
 *
 * Two agents measured this and disagreed — 3/3 kills against 2/2 clean, both
 * with plain in-place `cp`. Two hypotheses were live: the destination inode's
 * provenance (cp-created vs rename-created), and whether a live mapping of the
 * image exists at the moment of the write. This settles it, and additionally
 * tests the *remedy*, which neither hypothesis does.
 *
 * Measured on macOS (APFS), 2026-08-07, same two binaries throughout:
 *
 *   arm                                              dest created  overwrite   holder  killed
 *   A  in-place cp, nothing holding                  cp            cp          no      0/3
 *   B  in-place cp, nothing holding                  rename        cp          no      0/3
 *   C  in-place cp, require() held across the write  cp            cp          YES     3/3
 *   D  in-place cp, require() held across the write  rename        cp          YES     3/3
 *   E  rename,      require() held across the write  cp            rename      YES     0/3
 *
 * Conclusions:
 *   - The destination inode's provenance is NOT the discriminator: A and B are
 *     identical, and so are C and D.
 *   - A live mapping at the moment of the write IS: C and D kill, A and B do not.
 *   - Temp-file + rename survives the killing condition (E), and the mechanism
 *     is visible in the `inode changed` column — false in A-D, true in E. The
 *     holder keeps the old inode; the replacement is a different file.
 *
 * So: always stage with temp-file + rename. And a clean `cp`-staged run is not
 * evidence that anything is wrong — six trials here are exactly that.
 *
 * Usage:
 *   node scripts/dev/napi-staging-arms.mjs --a <binding-a.node> --b <binding-b.node>
 *
 * The two bindings must be genuinely different builds — the run aborts if they
 * are byte-equal, because identical binaries make every arm vacuously clean and
 * the harness could then only ever return "no kills". Produce a pair by
 * building the crate twice across any source change:
 *
 *   cargo build --release -p rsvelte_napi --lib && cp target/release/librsvelte_napi.dylib /tmp/A.node
 *   <edit something> && cargo build --release -p rsvelte_napi --lib && cp target/release/librsvelte_napi.dylib /tmp/B.node
 *
 * The `.node` extension is required: node parses a `.dylib` as JavaScript.
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync, spawnSync, spawn } from 'node:child_process';

const args = process.argv.slice(2);
const argOf = (flag) => {
	const i = args.indexOf(flag);
	return i !== -1 && args[i + 1] ? path.resolve(args[i + 1]) : null;
};

const A = argOf('--a');
const B = argOf('--b');
if (!A || !B) {
	console.error('usage: node scripts/dev/napi-staging-arms.mjs --a <binding-a.node> --b <binding-b.node>');
	process.exit(2);
}
for (const f of [A, B]) {
	if (!fs.existsSync(f)) {
		console.error(`[arms] missing ${f}`);
		process.exit(2);
	}
	if (!f.endsWith('.node')) {
		console.error(`[arms] ${f} must end in .node — node parses any other extension as JavaScript`);
		process.exit(2);
	}
}

// Identical binaries make the write a no-op and every arm vacuously clean: the
// harness would be incapable of reporting a kill. Refuse rather than measure a
// degenerate input.
if (fs.readFileSync(A).equals(fs.readFileSync(B))) {
	console.error('[arms] the two bindings are byte-identical — every arm would be vacuously clean');
	process.exit(2);
}

const STAGE = fs.mkdtempSync(path.join(os.tmpdir(), 'napi-arms-'));
const DEST = path.join(STAGE, 'rsvelte.node');

/** Load in a child process and report how it terminated. */
const loadOnce = (p) => {
	const r = spawnSync(process.execPath, ['-e', `require(${JSON.stringify(p)})`], { encoding: 'utf8' });
	if (r.signal) return `KILLED(${r.signal})`;
	if (r.status !== 0) return `EXIT(${r.status}) ${String(r.stderr).split('\n')[0].slice(0, 80)}`;
	return 'clean';
};

const place = (src, dest, how) => {
	if (how === 'rename') {
		execFileSync('cp', [src, `${dest}.tmp`]);
		execFileSync('mv', ['-f', `${dest}.tmp`, dest]);
	} else {
		execFileSync('cp', [src, dest]);
	}
};

function trial({ createBy, overwriteBy, holdOpen }) {
	fs.rmSync(DEST, { force: true });
	place(A, DEST, createBy);
	const inodeBefore = fs.statSync(DEST).ino;

	// Load once before the write. If this is ever anything but clean the binary
	// is bad and a later kill would not be attributable to the write.
	const first = loadOnce(DEST);

	let holder = null;
	if (holdOpen) {
		holder = spawn(process.execPath, ['-e', `require(${JSON.stringify(DEST)}); setTimeout(() => {}, 60000)`], {
			stdio: 'ignore',
		});
		execFileSync('sleep', ['1.5']); // let it actually map the image
	}

	place(B, DEST, overwriteBy);
	const inodeAfter = fs.statSync(DEST).ino;
	const second = loadOnce(DEST);
	if (holder) holder.kill('SIGKILL');

	return { first, second, inodeChanged: inodeBefore !== inodeAfter };
}

const ARMS = [
	{ id: 'A', desc: 'dest by cp,     overwritten in place by cp, nothing holding', createBy: 'cp', overwriteBy: 'cp', holdOpen: false },
	{ id: 'B', desc: 'dest by rename, overwritten in place by cp, nothing holding', createBy: 'rename', overwriteBy: 'cp', holdOpen: false },
	{ id: 'C', desc: 'dest by cp,     overwritten in place by cp, require() held  ', createBy: 'cp', overwriteBy: 'cp', holdOpen: true },
	{ id: 'D', desc: 'dest by rename, overwritten in place by cp, require() held  ', createBy: 'rename', overwriteBy: 'cp', holdOpen: true },
	{ id: 'E', desc: 'dest by cp,     REPLACED BY RENAME,         require() held  ', createBy: 'cp', overwriteBy: 'rename', holdOpen: true },
];
const N = 3;

console.log(`arm  ${'procedure'.padEnd(60)} killed  inode changed  first loads`);
let dirtyFirstLoad = false;
for (const arm of ARMS) {
	const rs = [];
	for (let i = 0; i < N; i++) rs.push(trial(arm));
	const killed = rs.filter((r) => r.second.startsWith('KILLED')).length;
	const inode = [...new Set(rs.map((r) => r.inodeChanged))].join('/');
	const firsts = [...new Set(rs.map((r) => r.first))].join('/');
	if (firsts !== 'clean') dirtyFirstLoad = true;
	console.log(`${arm.id}    ${arm.desc.padEnd(60)} ${String(killed)}/${N}     ${String(inode).padEnd(13)} ${firsts}`);
}

fs.rmSync(STAGE, { recursive: true, force: true });

// A kill is only attributable to the write if the binding loaded before it.
if (dirtyFirstLoad) {
	console.error('\n[arms] a pre-write load was not clean — the binaries are suspect and the arms are uninterpretable');
	process.exit(1);
}
console.log('\nall pre-write loads clean, so every kill above is attributable to the write.');
