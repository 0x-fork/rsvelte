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

### Two things the instrumentation says about itself

Signed residuals are load-bearing. Saturating them to zero hid an instrument
failure: on one project a child timer reads 2.2x its own parent, which is
impossible for sequential non-nested timers and means something is
double-counted. The mechanism is **unconfirmed**. Any share taken from the
legacy `$:` branch is suspect until it is explained.

The stage sum does not equal its parent. The residual is neither reproducible in
magnitude nor stable in sign -- between -0.05% and +2.5% of the parent on an
idle machine, flipping sign and reaching double digits under load. Read it as an
instrument reading, not as prologue time.
