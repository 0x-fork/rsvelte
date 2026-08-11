# Generated shape matrix — known failures

Ratchet for `scripts/compat-corpus/matrix/run.mjs` (#2281 Gate 2). Shrink-only and
two-sided: a new divergence fails CI, and so does a listed entry that already passes, so
the PR that fixes entries re-baselines in the same PR
(`node scripts/compat-corpus/matrix/run.mjs --update-baseline`).

## Why this gate exists

The collected corpus samples the **marginal** distribution of published Svelte code. Every
bug in the #2253/#2254/#2255/#2256 batch was an **interaction** — a binding kind × a
syntactic position, or a construct × a comment slot — and a found corpus under-samples
interactions exponentially:

| shape | occurrences in the 14,026-entry corpus |
|---|---|
| #2254 — `{#each … as X}` item as a `switch` discriminant | 0 |
| #2253 — `#private` `$state` assigned from a literal containing a `//` comment | 0 |
| #2256 — `svelte-ignore` before an object-literal property | 6 |

`client` and `server` were at **0 known failures** — saturated — when all four were
reported. Growing the corpus from 14k to 140k real files moves those counts from 0 to
approximately 0. Generating the product moves them to whatever the product contains.

## Scope of what a listed entry means

Normalization here is identical to `verify.mjs` (flatten template holes → oxfmt → strip
blank lines), so formatting-only differences are tolerated exactly as the corpus gate
tolerates them. An entry is a divergence that survives that.

## Matrix known failures (`matrix-known-failures.json`, 820 entries)

Partition of `matrix-known-failures.json` by family: `2 + 116 + 0 + 18 + 60 + 62 + 3 + 324 + 227 + 8`

### `binding-position` — 2 entries

Both are `label.body` on the **server** target (`derived-local`, `store-auto-sub`), and in
both **rsvelte's output is the correct one**.

`submodules/svelte/.../3-transform/server/visitors/LabeledStatement.js` returns early for
a non-`$` label **without calling `context.next()`**, so zimmerframe never descends into
the labeled subtree; the client visitor calls `context.next()` at the same guard. Since
`$.derived()` returns a function in `svelte/internal/server`, upstream emits
`if (doubled)` — always truthy — where every other position emits `doubled()`. Store
auto-subscriptions inside a labeled body are mis-emitted the same way. Reported upstream;
these two entries clear when the fix lands in `submodules/svelte`.

The rest of the family (7 bindings × 47 positions × 3 targets, minus these) passes. It is
the axis that found #2254 plus `SwitchCase.test`, class-expression field initializers and
class-expression computed method keys, all fixed in #2269.

### `comment-slot` — 116 entries

The `oxc_codegen` migration cleared 96 entries: all 72 trailing-comment failures on the
`.svelte.(js|ts)` module path and 24 server failures at the three interior slots of the
`module-script` seed. No module-path entries remain.

The remaining entries are `.svelte` template seeds. They are comment placement or
preservation differences only; normalized non-comment code matches.

| seed | entries |
|---|---:|
| `legacy-reactive` | 28 |
| `module-script` | 32 |
| `await-block` | 24 |
| `class-private-state` | 8 |
| `class-static-block` | 8 |
| `snippet-render` | 8 |
| `const-fold-line-continuation` | 8 |

Partition by target: `26 client + 26 client-dev + 64 server`.

Partition of `matrix-known-failures.json` entries under `comment-slot/` by what diverges: `56 + 32 + 28`

Partition of `matrix-known-failures.json` entries under `comment-slot/` by seed: `32 + 28 + 24 + 8 + 8 + 8 + 8`

### `each-collection` — 0 entries

Every collection shape now matches across all targets.

Partition of `matrix-known-failures.json` entries under `each-collection/` by collection: `0`

The axis this family exists for is at **zero**: every loose-binding collection (`??`, `||`,
`&&`, a ternary, `!x`, `typeof x`, `x + y`, a sequence, an assignment, `o?.list`) matches on all
three targets, and so does every tight-binding control (`list`, `o.list`, `o['list']`,
`(list)`). The `await list` rows are error-parity — both compilers reject them — which is 30 of
the family's comparisons and not a ratchet entry.

### `keyword-regex` — 18 entries

Not the family's own axis, and not its author's doing: these appear because this PR added
warning-**code** comparison to the gate, and `keyword-regex` is the one pre-existing family whose
inputs reach a warning. All 18 are one cause on all three targets —
`perf_avoid_nested_class` never fires for a `class` declared inside a legacy `$:` reactive
statement. The six cases are the `extends` row against every host and body that puts the class
there (`legacy-reactive`, `legacy-reactive-block`, and the four `body-*` rows, which run against
`legacy-reactive` by construction).

Partition of `matrix-known-failures.json` entries under `keyword-regex/` by target: `6 + 6 + 6`

Worth stating because it is the generalization argument for the comparison: a family written for
a *parser* question, by another author, with no warning intent, contributes 60 warned (case,
target) pairs and 18 divergences. The comparison earns its place on populations nobody built for
it.

### `param-pattern` — 60 entries

Every entry is the **legacy reactive dependency list**, not the statement body: rsvelte emits
`() => $.deep_read_state(rows())` where official emits
`() => ($.deep_read_state(rows()), $.deep_read_state(id()))`, and the same omission appears in
the `$.template_effect` deps array of the two markup contexts. The body text matches on all 180
cases; only the list of what the effect re-reads diverges, so the shipped symptom is a **lost
reactive dependency** — the statement does not re-run when the prop changes.

The rule rsvelte gets wrong is *which* identifiers inside a nested function count as reads. A
name in a **parameter default** or a **computed key** is a read (it is evaluated on every call,
in the enclosing scope), and upstream's `extract_all_identifiers` / scope resolution treats it as
one; rsvelte's extractor drops every identifier lexically inside a parameter list. Hence exactly
the five `read-` shapes whose name sits there fail, and `read-body` — the same read one bracket
later — passes:

| shape | in the ratchet |
|---|---|
| `({ k = id }) => k` | yes |
| `([k = id]) => k` | yes |
| `({ [id]: k }) => k` | yes |
| `(o = { id }) => o` | yes |
| `(o = [id]) => o` | yes |
| `(k) => k + id` | no — passes |

**[D]** It is not caused by the wrap fix this family shipped with, and the discriminating case is
`(o = id) => o`: a parameter default with no brackets at all, which
`is_destructured_param_binding` rejects at its first step and therefore cannot influence. rsvelte
omits `$.deep_read_state(id())` there too, official includes it. That shape is a control, not a
row — it is *only* a dependency-list case, with no pattern in it.

12 entries per shape: 6 of the 9 contexts reach a dependency list (four `$:` forms via
`$.legacy_pre_effect`, plus `interpolation` and `each-expression` via `$.template_effect`), each
on `client` and `client-dev`. `server` has no dependency list and matches everywhere.

Partition of `matrix-known-failures.json` entries under `param-pattern/` by shape: `12 + 12 + 12 + 12 + 12`

### `directive-element` — 62 entries

The remaining differences are limited to `bind:` validation on `<svelte:body>`,
`<svelte:document>`, and `<svelte:window>`, plus the two client-only `bind:this` outputs.

Partition of `matrix-known-failures.json` entries under `directive-element/` by verdict and host: `62`

**The `warning-missing:a11y_no_static_element_interactions` row — 24 entries on `svelte-element`
— is fixed by #2523 and no longer listed.** It read as one missing warning on four handler
spellings; it was the whole a11y pass, which had no call site in `svelte_element.rs`, so
`<svelte:element>` reached **none** of upstream's ~40 element rules. This family saw one of them
because `on:click` is the only a11y-relevant shape its axes construct. It is still the row that
justifies the warning comparison the family shipped with: a warning that never fires leaves the
output byte-identical, so `js.code` cannot report it.

Its verdict carried the **code**, and that was not cosmetic. With a flat `warning-mismatch`
verdict those 24 entries would have shared their ratchet key with every other warning on the same
case and target — and re-breaking #2521 (so `event_directive_deprecated` stops firing on
`<svelte:element>`) was measured to leave the gate **green**, because three of the four rows were
already listed. Keying on `warning-missing:<code>` / `warning-extra:<code>` makes that revert
produce 9 new ids instead, and is also what let #2523's fix be read off this gate as a clean
24 → 0 rather than as a change in a flat count.

### `bind-setter` — 3 entries

7 `bind:` expression shapes × 9 element kinds, 189 comparisons; all 3 entries are `client-dev`
and all 3 are the dev-mode `$.assign` wrap of #2484 — the family exists to make that defect's
element addressable.

| entry | direction |
|---|---|
| `plain__svelte-self` | rsvelte omits a wrap official emits |
| `getter-setter__svelte-body` | rsvelte emits a wrap official omits |
| `nested-arrow-in-setter__svelte-body` | rsvelte emits a wrap official omits |

Read this against how #2484 was reported: against `<svelte:component>`, which **matches** here,
while the live sites are `<svelte:body>` and `<svelte:self>`. The shapes the issue named
(`setter-through-call`, `sequence-bodied-setter`) all pass on the element and component hosts
now; what survives is the same predicate reached through a special element. A repro file cannot
find that, because the reporter picks the element.
### `removed-statement-comment` — 324 entries

The family crosses statements the SERVER transform removes (`$effect`, `$effect.pre`,
`$effect.root`, `$inspect`) with the comment slot (leading / interior / trailing), 6 comment
kinds, 3 hosts (`compileModule`, the instance script's top level, one function deep) and
whether a statement survives after the removed one. 396 cases, 1188 comparisons; the fix that
landed with it cleared 79 of them (403 → 324, all on `server`).

Every remaining entry falls in one of **four** clusters, each with its own issue. The clusters
are disjoint and exhaustive — the partition below sums to 324.

| entries | target | cluster | issue |
|---|---|---|---|
| 66 | `server` | `instance-top` × `succ-none` only: the removed statement is the last one in the script, so the orphaned comments have no anchor region to be re-homed onto. Upstream flushes them at the end of the enclosing function body; rsvelte's synthesized component-fn body is location-less, so esrap's closing `flush_comments_until` is a no-op | [#2716](https://github.com/baseballyama/rsvelte/issues/2716) |
| 108 | `client` | the `trailing` slot on `$effect` / `$effect.pre` / `$effect.root` (36 each, all 3 hosts × both successor states × 6 comment kinds): a comment trailing the call attaches to the effect's **callback argument** upstream, forcing esrap's wrapped one-argument-per-line layout; rsvelte attaches it after the call statement and keeps the call on one line. The comment survives — layout, not loss | [#2718](https://github.com/baseballyama/rsvelte/issues/2718) |
| 144 | `client-dev` | the same three statement kinds (36 each) **plus all 36 `$inspect` rows**. `$inspect` is the whole client/client-dev difference in this cluster: prod removes it, leaving only the 6 instance-top/no-successor rows below, while dev lowers it to a `console.log(…)` call and every trailing row then meets the same argument-wrapping rule | [#2718](https://github.com/baseballyama/rsvelte/issues/2718) |
| 6 | `client` | `inspect` × every comment kind in the `trailing` slot at instance top level with no successor: the orphaned comment is attached to the generated root declaration instead of the removed inspect statement's position | [#2737](https://github.com/baseballyama/rsvelte/issues/2737) |

Partition of `matrix-known-failures.json` entries under `removed-statement-comment/` by
cluster: `66 + 108 + 144 + 6`

**[D] for all four.** Each was reduced to a hand-written repro outside the family and measured
against the pinned official compiler. The `oxc_codegen` migration cleared #2736's four leading
block-comment rows; the six trailing rows now form one placement cluster.

**[S] on the pre-existing claim for the 258 client / client-dev entries.** They are argued
pre-existing structurally, not by an A/B: the fix that shipped with this family touches only
`3_transform/server/mod.rs` and `3_transform/server/ast/script.rs`, and the client target never
enters either. The server cluster *was* measured both ways — 145 entries with the fix reverted,
66 with it applied, on the same tree.

Note the enrolment cost, because it is real: a ratchet entry suppresses everything about the
entry it lists, so these 324 ids are now blind to any *further* regression on the same shapes
until their issues are fixed.

---

### `async-derived` — 227 entries

Added by #2540. Read the size as a **disclosure**, not a regression: not one of these 227 was
reachable by any gate in the repo before this family existed, because every harness compiles
with a fixed `{ generate, dev, filename }` and `$derived(await …)` is an `experimental_async`
compile error without `experimental.async`. The shape occurs 0 times in the 14k-entry corpus
and would occur 0 times in a 140k-entry one. This family is the first to make a compile
**option** an axis (`generate.mjs`'s `options`, merged in `run.mjs`), which is what turns the
shape from unreachable into measured.

The one thing #2540 itself fixed — the `label` / `location` arguments `$.async_derived` carries
in dev — is *not* in this list; the rows that isolate it (`instance__identifier__none`,
`instance__multi-declarator__none`, all three targets) pass. What remains are five independent
defects the family exposed on the way, all of them older than the family:

Partition of `matrix-known-failures.json` entries under `async-derived/` by cause: `128 + 39 + 18 + 14 + 13 + 13 + 2`

| # | cause | entries |
|---|---|---|
| 1 | `<script module>` / `compileModule` async-derived lowering | 130 |
| 2 | the `$$d` temp appears in the hoisted `var` list | 39 |
| 3 | `svelte-ignore` comment not reproduced on the hoisted declaration | 18 |
| 4 | a block comment before the declaration produces **invalid JavaScript** | 12 |
| 5a | no `$.save(…)` around a non-final `await` | 13 |
| 5b | `$derived.by(async …)` is suspended as if it were an async derived | 13 |
| — | server `$$renderer.async` split lost alongside cause 3 | 2 |

**1 — the module entry points.** Every `module__*` and `script-module__*` entry. The instance
script goes through the AST state transform; `<script module>` and `.svelte.js` go through the
module text pipeline, and that pipeline gets the dev lowering inside out — it emits
`(await $.track_reactivity_loss($.async_derived(() => p)))()` where upstream emits
`await $.async_derived(async () => (await $.track_reactivity_loss(p))(), 'a', '…')`, i.e. the
instrumentation wraps the *call* instead of the thunk body. Destructured module declarations
are not lowered at all (`const [a, b] = await $.async_derived(() => p)`), and the module
`server` target reads the derived without calling it (`return a` for `return a()`). Adding the
dev arguments to this path would have been invisible, so #2540 did not: the shape above it is
wrong first.

**2 — `var $$d, a, b;`.** rsvelte hoists its own destructuring temp into the component's
top-level `var` list; upstream keeps it local to the `$.run` callback. Present on `client`,
`server` and `client-dev` alike, so it is not dev instrumentation.

**3 and 4 — the ignore comment.** Upstream re-emits the `svelte-ignore` comment inside the
declaration it hoists (`var // svelte-ignore await_waterfall\n a;`); rsvelte drops it. Where the
comment is a block comment on the same line as the declaration, rsvelte does worse than drop it
— it splices it into the async hoist and produces
`$.run([async () => void (/* svelte-ignore await_waterfall */ const a = await …)])`, a `const`
in expression position that no JavaScript parser accepts. Cause 4 is a real bug, found by this
family, and the reason the `block-inline` slot is worth its 14 entries.

**Because of 3, the ignore axis cannot gate what it was added for.** A listed entry suppresses
everything about that entry, so a regression in the ignored form's argument list would not show
here. The assertions that do watch it are
`crates/rsvelte_core/tests/async_derived_dev_args_2540.rs` (exact argument list, three ignore
placements) and `scripts/compat-corpus/await-waterfall-runtime.mjs` (the warning actually
fires, and the ignore actually suppresses it). Clearing cause 3 hands the axis back to this
gate.

**5 — two lowering divergences the axis found incidentally.** A multi-`await` derived loses the
`$.save(…)` upstream wraps every non-final `await` in, and `$derived.by(async () => …)` is
routed through the async-derived hoist (`var a; $.run([…])` plus a `$$promises` blocker) where
upstream emits a plain `const a = $.derived(async () => …)` — rsvelte suspends on a derived
upstream does not.

## Burn-down

Re-baseline in the same PR as the fix:

```
cargo build --release -p rsvelte_napi --lib
mkdir -p .corpus-cache && cp target/release/librsvelte_napi.{dylib,so} .corpus-cache/rsvelte.node.staging && mv .corpus-cache/rsvelte.node.staging .corpus-cache/rsvelte.node
node scripts/compat-corpus/matrix/run.mjs --update-baseline
```

`--update-baseline` refuses to run under `--no-fmt` (which counts formatting-only
differences the corpus tolerates) or under a `--families` subset (which would delete every
baseline entry the run did not measure).
