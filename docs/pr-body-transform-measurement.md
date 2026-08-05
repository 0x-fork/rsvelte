# perf(transform): three byte-identical changes, plus the instrumentation that judged them

Reconstructed from the commit stack. The originally approved text lived only in
a conversation, so this file is the durable copy; check it against what you
approved before it is used.

Branch: `refactor/esrap-dead-api`. Not pushed.

## What ships

Three changes that alter allocation behaviour and nothing else:

- `3e16943f` key component slot grouping on borrowed names
- `52485f69` build module-source quoting in the arena
- `171e82c6` let the `clean_node_list` hoisted list grow on demand

Each carries its own deterministic counter commit measuring the work the
pre-change code would have done against the work actually done, with a
denominator, a negative control, and a `binary mtime > source mtime` check.

**No wall-clock claim is made for any of them.** The measured allocation saving
across the stack is about 86 allocations per file, which against a ~3.8ms
compile is between 0.07% and 0.23% -- below what a paired A/B can resolve. They
are hygiene, not a performance result. Reviewers should read the counters as
"this many fewer allocations", never as "this much faster".

## Byte identity

Output is unchanged, established by a raw-byte sweep rather than by tests
passing:

- interval: `a3f446ee` (the parent of the await-scanner change) through HEAD
- `diff -r` over the corpus output trees: **exit 0, 0 lines**
- **42,454 files / 104,352,444 bytes**, both trees equal
- the output tree is copied out with `cp -Rc` *before* `verify.mjs` runs, because
  verify rewrites it in place with oxfmt and that would destroy the raw evidence
- `binary mtime > source mtime` checked on both builds
- **negative control on `diff` itself**: one byte injected into one file,
  detected (exit 1), reverted, re-run clean (exit 0)

The corpus verify pipeline is *not* the evidence here: it normalises both sides
through oxfmt and falls back to acorn AST equivalence, and it compares rsvelte
against the official compiler rather than before against after. It cannot see a
raw-byte change of this kind.

## The instrumentation

The rest of the stack is measurement, kept because it is what the decisions were
made on:

- `87578b1a` / `f355e937` a same-run time-share profile over the shipped-source corpora
- `848a52ad` how to rebuild that profile, including the three traps that make a
  sampler lie under fat LTO
- `698ab455` the per-prop re-scan in `transform_prop_reads_in_expr`, measured at
  a re-scan factor of 11.60x -- and then *not* acted on, because the same
  measurement put it under the pre-registered 3% threshold
- `a94f5a58` / `9aa956fe` / `da72704e` the script-text bucket split into stages
  and down to per-statement branches
- `4e3da96f` residuals printed signed rather than saturated to zero
- `e2bdd1c0` / `249e5538` the same profile over shipped sources, per project
- `1a82bc2f` / `0f012ab5` the re-parse probe, covering both the shared driver
  and the ten passes that build a parser themselves

`ccb07fa0` (the esrap call-site split) is included here: it is instrumentation
of the same kind and is not on `main`.

### What these numbers are, and are not

Every share here was taken **before the in-place flip** (`b49fcc3e`, which is
not in this branch). The flip makes passes parse and print through
`with_program_mut`, work these measurements do not contain -- so the re-parse
figure is a floor for the shipping compiler, not its value. Re-measuring after
the flip is separate work.

The shipped-source shares were taken **with** the shadowed-rune indexing fix
(`e3d98dc8`, PR #2092) already in the branch. Anything superlinear reported here
is what remains after that fix, not what it addressed.

Populations are named wherever a number appears, because they disagree with each
other: the svelte test corpus carries `$:` at roughly three times the density of
shipped code (130/3,874 against 71/5,879, five of six shipped projects at zero).
Conclusions drawn on rune-only shipped code are stated as such and do not extend
to legacy input.

### Two things the instrumentation says about itself

Signed residuals are load-bearing. Saturating them to zero hid an instrument
failure: on one project a child timer reads 2.2x its own parent, which is
impossible for sequential non-nested timers. Three explanations were tested and
falsified by counters -- re-entry, unpaired call sites, and entries outside any
parent interval were all zero -- and a fourth counter timing the function from
inside showed the stage timers agree with it while the parent under-reports. So
the direction is settled and the mechanism is **unconfirmed**. Shares taken from
the legacy `$:` branch are understated by the parent, not inflated.

The stage sum does not equal its parent. The residual is neither reproducible in
magnitude nor stable in sign -- between -0.05% and +2.5% of the parent on an
idle machine, flipping sign and reaching double digits under load. Read it as an
instrument reading, not as prologue time.
