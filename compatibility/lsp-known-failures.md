# lsp-known-failures.json — why entries are accepted (language-server parity)

The LSP parity gate (`scripts/compat-corpus/lsp-verify.mjs`) drives the pinned real
`svelte-language-server` (`scripts/compat-corpus/lsp-oracle`) and `rsvelte-language-server` over
stdio with the same `initialize` params, the same client capabilities and the same request
stream, and records every response pair that does not agree. The ratchet may only shrink, and it
is two-sided: a new divergence **and** a listed entry that no longer diverges both fail CI.

Entry format: `<project>/<file>|<method>|<verdict>`.

The verdict is a **class**, not a payload:

| verdict | meaning |
|---|---|
| `only-official` | official answered, rsvelte answered nothing (`null`, `[]`, an empty token stream) |
| `only-rsvelte` | the reverse |
| `count` | both answered, with different numbers of items |
| `differs:<fields>` | both answered the same number of items, differing at up to three named fields |
| `error-official:<code>` / `error-rsvelte:<code>` | one side returned a JSON-RPC error, or timed out |
| `error-both:<a>/<b>` | both errored, with different codes |

Keying on the payload would churn on every TypeScript wording change; keying on
`<project>/<file>|<method>` alone would let a divergence that changes *kind* reuse an existing
entry, which is the failure mode #2521 recorded for the shape matrix. Putting the class in the
key is the compromise, and its cost is stated as blind spot 27a in
[gate-coverage.md](gate-coverage.md): once an entry exists, the payload inside that class is no
longer compared.

## Current baseline: `lsp-known-failures.json`, 3044 entries

Read the denominator with it. The gate compared **7231 response pairs across 227 units** in 11
projects, and **2804 (38.8%) already agree**. This is a from-scratch Rust language server against
a 27,000-line TypeScript one; the ratchet is a burndown list, not a defect list — a large part of
it is one deliberate design difference reproduced per file.

### Partition by project

| project | entries | agreement |
|---|---|---|
| `upstream/diagnostics` | 450 | 433/1262 |
| `corpus/flowbite-svelte` | 429 | 300/825 |
| `corpus/shadcn-svelte` | 419 | 313/824 |
| `corpus/bits-ui` | 403 | 305/827 |
| `corpus/melt-ui` | 336 | 267/704 |
| `upstream/testfiles` | 332 | 515/1135 |
| `upstream/inlay-hints` | 283 | 184/580 |
| `upstream/folding-range` | 135 | 240/474 |
| `upstream/svelte-plugin` | 118 | 98/269 |
| `fixtures/native` | 91 | 120/232 |
| `fixtures/basic` | 48 | 29/99 |

Partition of `lsp-known-failures.json` by project: `450 + 429 + 419 + 403 + 336 + 332 + 283 + 135 + 118 + 91 + 48`

The four `corpus/*` projects are the real-world repositories the compiler corpus already pins,
sampled 25 components each; `upstream/*` are the fixture suites of
`submodules/language-tools`; `fixtures/*` are committed mini-projects.

### Partition by method

| method | entries | why it is where it is |
|---|---|---|
| `hover` | 386 | 163 `only-official` (rsvelte has nothing at that position), 112 `differs:character,contents,end` (both answer, over different ranges — e.g. official answers MDN prose for an HTML element name where rsvelte answers a TypeScript hover for the generated `$$render`), 97 `differs:contents` alone (same range, different text; many are a single trailing newline) |
| `selectionRange` | 363 | 187 `only-official`; the rest differ in the `parent` chain — the chain of enclosing ranges is built independently on both sides and one extra or missing level differs the whole chain. 22 are the `error-official` class below |
| `completion` | 355 | 220 `only-official`, 127 `count`. Compared as the sorted `label kind insertTextFormat` set, so these are different item sets, not ordering (27d) |
| `documentHighlight` | 333 | 191 are `differs:kind` alone — the same ranges, classified read/write/text differently |
| `definition` | 218 | 189 `only-official` — positions rsvelte routes nowhere (a component tag, an HTML element name) |
| `documentSymbol` | 217 | 127 `count` and 60 `differs:character,children,end` — which markup nodes become symbols, and how deeply they nest |
| `formatting` | 199 | 169 `differs:newText`. rsvelte formats with `rsvelte-fmt`, official with prettier + `prettier-plugin-svelte`; byte parity of the two formatters is the **fmt-parity gate's** subject, not this one, and these entries restate it once per file |
| `publishDiagnostics[rsvelte]` | 181 | **a deliberate divergence**: rsvelte's server also publishes its native linter's findings, which official has no counterpart for. Kept in the key rather than filtered out so it cannot mask the TypeScript ones |
| `semanticTokens/full` | 166 | rsvelte returns an empty token stream where official returns tokens — see below |
| `foldingRange` | 156 | 145 `count` — which regions fold at all, rather than where they start and end |
| `codeLens` | 139 | both sides emit **unresolved** lenses (the title arrives from `codeLens/resolve`, which this gate never calls — 27e), so what diverges is how many there are and where |
| `publishDiagnostics[ts]` | 129 | the TypeScript diagnostics of the overlay: different overlay text produces different errors, and the two sides are on different TypeScript majors (27i) |
| `typeDefinition` | 91 | same shape as `definition` |
| `inlayHint` | 45 | parameter-name and type hints from the overlay |
| `publishDiagnostics[svelte]` | 37 | compiler warnings and errors, the smallest group — this is the part both sides get from the same compiler port |
| `publishDiagnostics[js]` | 23 | official's JS-mode diagnostics on `.js`/`checkJs` files |
| `documentColor` | 4 | the smallest group of all: colour literals in `<style>` |
| `publishDiagnostics[rsvelte-css]` | 2 | rsvelte's native CSS diagnostics |

Partition of `lsp-known-failures.json` by method: `386 + 363 + 355 + 333 + 218 + 217 + 199 + 181 + 166 + 156 + 139 + 129 + 91 + 45 + 37 + 23 + 4 + 2`

### Partition by verdict class

| verdict class | entries |
|---|---|
| `only-official` | 1166 |
| `differs` | 959 |
| `count` | 577 |
| `only-rsvelte` | 320 |
| `error-official` | 22 |

Partition of `lsp-known-failures.json` by verdict class: `1166 + 959 + 577 + 320 + 22`

`only-official` being the largest class is the honest summary of where the port is: the most
common divergence is rsvelte having nothing to say at a position where official does. Of the 320
`only-rsvelte` entries, 181 are the native linter's `publishDiagnostics[rsvelte]` — a feature
official has no counterpart for at all — followed by 45 `inlayHint`, 26 `formatting` and 17
`documentHighlight`.

`error-official` (22) is the one class where **official** is the outlier: it answers
`textDocument/selectionRange` on a `.ts` document with an internal error (`-32603`), which
rsvelte does not.

## The one class worth calling out separately

`semanticTokens/full`, 166 entries, all `only-official`: rsvelte returns `{ "data": [] }` where
official returns a token stream. This was **invisible until the run that produced this file**.
The gate's projection dropped the `data` key everywhere as the opaque payload of a
`completionItem/resolve` round trip — and `data` is also where a semantic-tokens response keeps
its entire answer, so every token stream compared equal to every other and the gate reported zero
semantic-token divergences over all 227 files. `scripts/dev/test-lsp-normalize.mjs` now fails if
a projection erases the field under comparison; it runs in `ci.yml` without needing either
server.

## What is NOT in this file

The gate carries a **positive control on its own oracle**, and it is fatal rather than
ratcheted: for the 15 `upstream/folding-range` fixtures, the official server's response over
stdio is compared against upstream's own `expectedv2.json`. It reproduces **13 of 15**; the two
misses are one extra whole-`<script>` fold each, contributed by the HTML folding provider that
upstream's provider-level test does not run. Below 0.8 the run aborts, because a run measured
against a server that is not behaving as its own tests say would produce a plausible ratchet full
of meaningless entries.

The same reasoning covers a project that agrees on **nothing** — that is one server failing to
start, not 800 divergences — and `--update` below 150 compared units.
