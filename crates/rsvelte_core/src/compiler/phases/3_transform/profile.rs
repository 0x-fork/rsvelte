//! Lightweight thread-local timers for splitting Phase 3 (Transform) into
//! sub-phases (template fragment walk, instance-script text transform, CSS
//! render, JS codegen).
//!
//! ## What the instrumentation costs, and why it is off by default
//!
//! Only the native profiling binaries read these accumulators back — the
//! compile pipeline never does, so the timers cannot affect compiler output.
//! What they can affect is how long it takes.
//!
//! Measured 2026-08-05 by `rsvelte_devtools/bin/timer_pair_cost.rs`, which runs
//! these very functions in a loop rather than comparing two builds:
//!
//! ```text
//! one clock read                      18.7 ns
//! recorder (thread-local + Cell)       6.7 ns
//! one timer pair                      44.2 ns   (three recorder shapes agree to 0.3%)
//! gate load and branch                 0.33 ns  (measured below)
//! one gated pair                      44.5 ns
//! pairs per compile                  101.8      legacy corpus, 633 files
//! per compile                          4.5 µs = 0.198% of a 2292 µs compile
//! ```
//!
//! The gated figure is the sum of two measurements rather than a third one. Both
//! terms are measured, and adding them cannot pick up the layout difference or
//! the machine load that a fresh whole-run comparison would.
//!
//! An earlier version of this note claimed ~100–200ns per file. That was off by
//! a factor of ~25, because it counted a handful of sites rather than the 101.8
//! pairs a compile actually reaches. The rune corpus pays 24.8 pairs, or 0.117%.
//!
//! 0.198% is above the cut we set for shipping timers unconditionally, so
//! [`set_timers_enabled`] gates them and they start off. The gate is a relaxed
//! atomic load, so both states stay in one binary and the profiled build is the
//! shipped build.
//!
//! What a shipped compile still pays is the gate itself, measured the same way:
//!
//! ```text
//! gate load and branch                 0.33 ns per site
//! per compile                          0.034 µs = 0.0015%
//! ```
//!
//! so the gate removes 99.5% of the direct cost. The same run checks that no
//! recorder accumulated while the gate was shut, since a recorder the gate did
//! not reach would corrupt every later profile rather than merely cost time.
//!
//! What this does NOT cover: the cost of the instrumentation merely existing
//! (instruction-cache pressure, inlining decisions elsewhere). A runtime gate
//! cannot remove that, since the code stays in the binary, so it is unmeasured
//! and its sign is unknown. The wall-clock A/B that would have measured it had a
//! best-case spread of 8.09% against a 0.3% question and was retired.
//!
//! Consumed by the `rsvelte_devtools` profiling binaries, which enable the gate
//! at startup.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// `std::time::Instant::now()` traps on `wasm32-unknown-unknown` (no system
// clock — see std::sys::time::unsupported). The profile instrumentation
// below is consumed only by the native profiling binaries, but the call
// sites live in shared compile paths, so the Instant calls would
// fire from the WASM playground and crash the page. Provide a WASM-safe
// shim that returns a unit "instant" with a zero-cost elapsed so the
// instrumented sites stay compile-target-portable without #[cfg] noise.

/// Whether the phase timers read the clock. See the module note for the 0.198%
/// that makes this a gate rather than an unconditional cost.
///
/// Relaxed is enough: nothing is published through this flag, and a profiling
/// run that missed the first few compiles would still be a valid sample of the
/// rest. Making it `Acquire`/`Release` would buy ordering no reader needs.
static TIMERS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn the phase timers on or off for the whole process.
///
/// Profiling binaries call this at startup. Leaving it off is what the shipped
/// compiler does, and the two states differ only in a branch, so there is no
/// build in which the timers exist and no build in which they are free.
///
/// Every recorder below, and every timer guard's `Drop`, opens by returning on
/// this. The check comes before the thread-local access rather than around the
/// accumulate because the thread-local is the expensive half: a shut gate
/// costs 0.33ns against 44.5ns for a timed pair, which is where the 99.5% in
/// the module note comes from.
pub fn set_timers_enabled(on: bool) {
    TIMERS_ENABLED.store(on, Ordering::Relaxed);
}

#[inline]
pub fn timers_enabled() -> bool {
    TIMERS_ENABLED.load(Ordering::Relaxed)
}

#[cfg(not(target_arch = "wasm32"))]
pub type TimerStart = Option<std::time::Instant>;

#[cfg(target_arch = "wasm32")]
pub type TimerStart = ();

#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub fn timer_start() -> TimerStart {
    if !timers_enabled() {
        return None;
    }
    #[cfg(feature = "measure-timer-calls")]
    TIMER_STARTS.with(|c| c.set(c.get() + 1));
    Some(std::time::Instant::now())
}

#[cfg(target_arch = "wasm32")]
#[inline]
pub fn timer_start() -> TimerStart {}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub fn timer_elapsed(start: TimerStart) -> Duration {
    // `None` means the gate was off when the timer started. Returning the
    // elapsed time of a clock read that never happened is not an option, and
    // zero is what every recorder already treats as "no contribution".
    start.map_or(Duration::ZERO, |start| start.elapsed())
}

#[cfg(target_arch = "wasm32")]
#[inline]
pub fn timer_elapsed(_start: TimerStart) -> Duration {
    Duration::ZERO
}

// How many `Instant` pairs one compile pays for.
//
// The unit is deliberately `timer_start`, not `record_*`: the recorders take
// between zero and two `Duration`s, so counting them would mix sites that read
// the clock twice with sites that never read it. Every `timer_start` is one
// `now()` plus the `elapsed()` that consumes it, so dividing the measured
// instrumentation cost by this count gives a per-pair figure without assuming
// the sites are alike.
#[cfg(feature = "measure-timer-calls")]
thread_local! {
    static TIMER_STARTS: Cell<u64> = const { Cell::new(0) };
}

#[cfg(feature = "measure-timer-calls")]
pub fn take_timer_starts() -> u64 {
    TIMER_STARTS.with(|c| c.replace(0))
}

/// Per-call-site cost of the `rsvelte_esrap` printer inside one compile run.
///
/// Split out from [`Phase3Breakdown`] because the printer is reached from six
/// independent sites and the question "would a faster printer speed up
/// `compile()`" needs each site's share separately, not their sum. The three
/// client branches stay apart because they take different printer entry points
/// and only one of them is reachable per compile.
#[derive(Default, Debug, Clone, Copy)]
pub struct EsrapBreakdown {
    /// Client final print, comment-bearing branch (`print_split`).
    pub client_split: Duration,
    pub client_split_calls: u64,
    /// Client final print, sourcemap branch (`print_with_map`).
    pub client_map: Duration,
    pub client_map_calls: u64,
    /// Client final print, plain branch (`print_with`).
    pub client_plain: Duration,
    pub client_plain_calls: u64,
    /// Server final print (the single whole-program `print`).
    pub server_print: Duration,
    pub server_print_calls: u64,
    /// Server async-body round-trip: the print half.
    pub server_pipe_print: Duration,
    /// Server async-body round-trip: the re-parse half of the same round-trip.
    pub server_pipe_reparse: Duration,
    pub server_pipe_calls: u64,
    /// `normalize_js_with_oxc` slow path (parse + print), print half only.
    pub normalize_print: Duration,
    pub normalize_calls: u64,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Phase3Breakdown {
    pub visit_program: Duration,
    pub script_text_transform: Duration,
    pub template_fragment: Duration,
    pub assembly_after_fragment: Duration,
    pub css_render: Duration,
    pub codegen: Duration,
}

/// One level below [`Phase3Breakdown::script_text_transform`], which is the
/// largest Phase 3 bucket. The five stages are sequential and disjoint, so the
/// difference between their sum and `script_text_transform` is the prologue
/// plus the early-out paths.
///
/// Do not expect `Σ stages == script_text_transform`. The residual is small
/// against the parent but is neither reproducible in magnitude nor stable in
/// sign -- measured between -0.05% and +2.5% of the parent on an idle machine,
/// and it flips sign and grows to double digits under load. The cause is
/// unexplained; read the stage shares as good to roughly a point, and read the
/// residual as an instrument reading rather than as prologue time.
#[derive(Default, Debug, Clone, Copy)]
pub struct ScriptTextBreakdown {
    /// Comment strip, class fields, comma split, arrow-paren strip.
    pub prenormalize: Duration,
    /// Gathering reactive / proxy / prop variable sets from the script text.
    pub collect_vars: Duration,
    /// The line-by-line accumulation loop, `process_accumulated` included.
    pub line_loop: Duration,
    /// The part of `line_loop` spent transforming completed statements; the
    /// remainder is the loop's own per-line scanning.
    pub process_accumulated: Duration,
    /// Completed statements handed to the processor.
    pub statements: u64,
    /// `transform_client_runes_with_skip_and_state`, the per-statement rune rewrite.
    pub runes: Duration,
    /// The legacy `$:` reactive-statement branch.
    pub reactive_stmt: Duration,
    pub reactive_calls: u64,
    /// Reactive-statement append plus the runes-mode AST transforms.
    pub ast_transforms: Duration,
    /// Shadowed-local post-pass and dev-mode instrumentation.
    pub post_passes: Duration,
    /// Calls that reached the staged region (i.e. did not take an early out).
    pub calls: u64,
    /// Every entry into the staged function, early outs included.
    pub entries: u64,
    /// Every `record_script_text`. Must equal `entries`, or some staged work
    /// ran outside the parent timer and the stage sum cannot be compared to it.
    pub parent_calls: u64,
    /// Entries that happened while an outer entry was still on the stack.
    ///
    /// `entries == parent_calls` does not prove the two are paired: a missing
    /// one and a spare one cancel. Re-entry is the case that actually breaks
    /// the arithmetic, because the inner call's stage timers accumulate into
    /// the same totals the outer call's parent timer already covers, letting
    /// the stage sum exceed its parent. Any non-zero value here invalidates
    /// every share taken from `script_text` and below.
    pub nested_entries: u64,
    /// `record_script_text` split by call site, so a parent call with no entry
    /// (or the reverse) cannot hide inside an equal total.
    pub parent_site_main: u64,
    pub parent_site_pub: u64,
    /// Wall time inside the staged function, measured by the entry guard.
    ///
    /// Bounds the stage sum from above and the parent timer from below, so
    /// when the two disagree this says which of them is wrong.
    pub in_function: Duration,
    /// Entries that ran while no parent interval was open.
    ///
    /// This is the case equal totals cannot rule out: a parent interval that
    /// wraps no call, paired with a call that no parent wraps.
    pub entries_outside_parent: u64,
}

/// The `*_ast` passes reach the parser through two entry points in
/// `super::shared::ast_rewrite` -- `with_program` for the eight that only read
/// a program, `with_program_mut` for the ten that rewrite one -- and both are
/// counted here.
///
/// `bytes` is the load-independent quantity: the total source length handed to
/// the parser across a compile. Divided by the length of *what was handed to
/// it*, the script text, it says how many times over the pipeline re-reads the
/// same script, and a ratio that grows with size is superlinear re-parsing.
///
/// Two earlier claims in this note were wrong, and both inflate confidence
/// rather than merely blur it. It said to divide by the file length: that
/// denominator includes template and style bytes the parser never sees, so a
/// template-heavy file scores low for having a large denominator rather than
/// for being re-parsed less. And it said one choke point covered every pass,
/// while `with_program_mut` parsed without recording -- so any re-parse figure
/// taken before this was the reading passes only.
#[derive(Default, Debug, Clone, Copy)]
pub struct ReparseBreakdown {
    /// Time inside `Parser::parse` only.
    pub parse: Duration,
    /// Time inside the visitor closure, i.e. everything the pass does with the
    /// program once it exists.
    pub visit: Duration,
    pub calls: u64,
    /// Summed `source.len()` over every call.
    pub bytes: u64,
    /// The same three numbers for passes that build a `Parser` themselves
    /// instead of going through the shared driver. Kept apart so the driver's
    /// count is never mistaken for the whole re-parse cost.
    pub direct_parse: Duration,
    pub direct_calls: u64,
    pub direct_bytes: u64,
}

/// The whole of `compile()`, split at the boundaries the production pipeline
/// actually has.
///
/// Recorded from inside the pipeline rather than by a driver that calls the
/// phase functions itself. A driver has to thread the retained scripts, call
/// every step, pick the same implementation, and pass the same argument
/// values — four things it can get wrong, and the older profilers got all four
/// wrong at once. Timing the production call sites removes the notion of
/// "calling it correctly": whatever `compile()` does is what is measured.
///
/// `total` is taken around the entire compile, so `total - Σ(buckets)` is time
/// no bucket claims. Report that residual. A bucket nobody added shows up
/// there instead of inflating the shares of the buckets that do exist, which
/// is how TypeScript removal stayed missing from the older split unnoticed.
#[derive(Default, Debug, Clone, Copy)]
pub struct PipelineBreakdown {
    // ## Folding these into another compiler's buckets
    //
    // Written here rather than at a call site because both the devtools binary
    // and the napi readout hand these numbers to a comparison, and a fold
    // decided per consumer would let the same bucket mean two things.
    //
    // ```text
    // parse           ->  parse
    // analyze         ->  analyze
    // transform       ->  transform + codegen + css
    // everything else ->  no fold
    // ```
    //
    // `transform` covers codegen and CSS rendering because both happen inside
    // `transform_component_with_scripts`: the client transform returns a
    // finished code string, and `record_css_render` fires from that same call.
    // A comparison against a compiler that separates them adds its own parts
    // together rather than expecting this side to split.
    //
    // It can be split, but only part way. [`Phase3Breakdown::codegen`] is a
    // sub-bucket of `transform`, and every site recording it is on the client
    // path -- the server transform has none. A client-only run can report
    // codegen separately; a server or mixed run would report a smaller codegen
    // rather than no codegen, which is the worse failure of the two.
    //
    // The unfoldable rows are not necessarily extra work. Another compiler may
    // do the same thing somewhere its buckets do not name it, so "no fold"
    // means the correspondence is unknown, not that the work is unique.
    /// Template parse, deferred script split included.
    pub parse: Duration,
    /// The one full-source scan that produces line offsets.
    pub line_offsets: Duration,
    /// Parsing the template expressions phase 1 deferred.
    pub resolve_lazy: Duration,
    /// OXC over the instance and module scripts, retaining the programs.
    pub ensure_script: Duration,
    /// TypeScript node removal. Absent from every older profiler.
    pub ts_removal: Duration,
    /// Merging `<svelte:options>` into the compile options.
    pub options_merge: Duration,
    /// Phase 2. Its own sub-split is not covered here.
    pub analyze: Duration,
    /// Phase 3, whose sub-split is [`Phase3Breakdown`].
    pub transform: Duration,
    /// Assembling the `CompileResult` once Phase 3 has produced its output.
    pub finalize: Duration,
    /// The entire compile, measured independently of the buckets above.
    pub total: Duration,
    /// Compiles that reached the pipeline, for per-file figures.
    pub compiles: u64,
}

impl PipelineBreakdown {
    /// Time inside `total` that no bucket claims. A missing bucket lands here.
    pub fn unattributed(&self) -> Duration {
        self.total.saturating_sub(
            self.parse
                + self.line_offsets
                + self.resolve_lazy
                + self.ensure_script
                + self.ts_removal
                + self.options_merge
                + self.analyze
                + self.transform
                + self.finalize,
        )
    }
}

/// The analyze phase, split at the call boundaries `analyze_component` has.
///
/// The parent is [`PipelineBreakdown::analyze`] -- the same counter, not a
/// second timer around the same code, so the two cannot disagree by drift.
/// That is also why [`take_analyze_breakdown`] **peeks** the parent while it
/// **takes** the buckets; see that function for the ordering it forces.
///
/// The buckets are the named calls only. `analyze_component` also does a large
/// amount of inline work between them -- rune and await detection, the module
/// and instance walks, the `{#each}` scan -- and none of it is bucketed. That
/// work lands in [`AnalyzeBreakdown::unattributed`], which is the point: a
/// phase whose residual is most of its total is a phase whose cost is not in
/// the calls anyone would think to name.
///
/// `extract_scripts` is instrumented even though it runs before the phase's
/// first analysis step, because leaving it out would make the residual mean
/// two things at once -- work nobody bucketed, and one specific call we chose
/// not to.
#[derive(Default, Debug, Clone, Copy)]
pub struct AnalyzeBreakdown {
    /// `Analysis::extract_scripts` -- carves out the script text Phase 3 reuses.
    pub extract_scripts: Duration,
    pub extract_scripts_calls: u64,
    /// `Analysis::create_scopes`.
    pub create_scopes: Duration,
    pub create_scopes_calls: u64,
    /// `store_subscriptions::detect_store_subscriptions`.
    pub store_subs: Duration,
    pub store_subs_calls: u64,
    /// `visitors::analyze_template`.
    pub template: Duration,
    pub template_calls: u64,
    /// Template nodes dispatched by the two walks, counted at their single
    /// `visit_node` each -- `ScopeBuilder::visit_node` for `create_scopes`,
    /// `visitors::visit_node` for `template`.
    ///
    /// **These count template nodes only, and are not a per-node denominator
    /// for their buckets.** Both walks also descend into the scripts, and
    /// analyze has no central hook for JS nodes: the traversal goes through
    /// `get_js_children` at roughly fifty sites across eight files, so a JS
    /// count would mean touching every one of them, and counting calls there
    /// would count parents rather than nodes. For a script-heavy component the
    /// uncounted part is most of the work, so `time / nodes` here is a walk
    /// density, not a cost per node.
    pub create_scopes_nodes: u64,
    pub template_nodes: u64,
    /// `Analysis::analyze_css` and `css::analyze::analyze_css_with_source`:
    /// the CSS analysis and validation pass.
    pub css_analyze: Duration,
    pub css_analyze_calls: u64,
    /// The rest of the `ast.css` block -- selector-info extraction, pruning,
    /// and the element scoping walk. Split from `css_analyze` because the two
    /// answer different questions (is this CSS valid, versus which elements
    /// does it touch), and kept adjacent to it so that between them they cover
    /// the whole block: no CSS call falls into the residual.
    pub css_scope: Duration,
    pub css_scope_calls: u64,
    /// [`PipelineBreakdown::analyze`], read without clearing it.
    pub total: Duration,
    /// Compiles that reached the pipeline, for per-file figures.
    pub compiles: u64,
}

impl AnalyzeBreakdown {
    /// Time inside `total` that no bucket claims -- here, the inline work
    /// between the named calls. Expected to be large; report it rather than
    /// folding it into a neighbour.
    pub fn unattributed(&self) -> Duration {
        self.total.saturating_sub(
            self.extract_scripts
                + self.create_scopes
                + self.store_subs
                + self.template
                + self.css_analyze
                + self.css_scope,
        )
    }
}

thread_local! {
    static PL_PARSE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_LINE_OFFSETS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_RESOLVE_LAZY: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_ENSURE_SCRIPT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_TS_REMOVAL: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_OPTIONS_MERGE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_ANALYZE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_TRANSFORM: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_FINALIZE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_TOTAL: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PL_COMPILES: Cell<u64> = const { Cell::new(0) };

    // Analyze sub-split. Each carries its own call count so that a zero can be
    // read: `0ns / 0 calls` is a branch that never ran, `0ns / n calls` is work
    // below the clock, and only the first is "not measured".
    static AN_EXTRACT_SCRIPTS: Cell<(Duration, u64)> =
        const { Cell::new((Duration::ZERO, 0)) };
    static AN_CREATE_SCOPES: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static AN_STORE_SUBS: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static AN_TEMPLATE: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static AN_CSS_ANALYZE: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static AN_CSS_SCOPE: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };

    // Deterministic node counts, load-independent: one run is enough. Gated by
    // the same switch as the timers so the shipped compiler pays nothing.
    static AN_SCOPE_NODES: Cell<u64> = const { Cell::new(0) };
    static AN_TEMPLATE_NODES: Cell<u64> = const { Cell::new(0) };

    static REPARSE: Cell<(Duration, Duration, u64, u64)> =
        const { Cell::new((Duration::ZERO, Duration::ZERO, 0, 0)) };
    static REPARSE_DIRECT: Cell<(Duration, u64, u64)> =
        const { Cell::new((Duration::ZERO, 0, 0)) };

    static VISIT_PROGRAM: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static SCRIPT_TEXT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static TEMPLATE_FRAGMENT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ASSEMBLY_AFTER_FRAGMENT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CSS_RENDER: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CODEGEN: Cell<Duration> = const { Cell::new(Duration::ZERO) };

    static ST_PRENORMALIZE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_COLLECT_VARS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_LINE_LOOP: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_AST_TRANSFORMS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_POST_PASSES: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_CALLS: Cell<u64> = const { Cell::new(0) };
    static ST_ENTRIES: Cell<u64> = const { Cell::new(0) };
    static ST_PROCESS_ACCUMULATED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_STATEMENTS: Cell<u64> = const { Cell::new(0) };
    static ST_RUNES: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_REACTIVE_STMT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_REACTIVE_CALLS: Cell<u64> = const { Cell::new(0) };
    static ST_PARENT_CALLS: Cell<u64> = const { Cell::new(0) };
    static ST_DEPTH: Cell<u64> = const { Cell::new(0) };
    static ST_NESTED_ENTRIES: Cell<u64> = const { Cell::new(0) };
    static ST_IN_FUNCTION: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_PARENT_OPEN: Cell<u64> = const { Cell::new(0) };
    static ST_ENTRIES_OUTSIDE: Cell<u64> = const { Cell::new(0) };
    static ST_PARENT_SITE_MAIN: Cell<u64> = const { Cell::new(0) };
    static ST_PARENT_SITE_PUB: Cell<u64> = const { Cell::new(0) };

    static ESRAP_CLIENT_SPLIT: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static ESRAP_CLIENT_MAP: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static ESRAP_CLIENT_PLAIN: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static ESRAP_SERVER: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static ESRAP_PIPE: Cell<(Duration, Duration, u64)> =
        const { Cell::new((Duration::ZERO, Duration::ZERO, 0)) };
    static ESRAP_NORMALIZE: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
}

#[inline]
pub fn record_reparse(parse: Duration, visit: Duration, bytes: usize) {
    if !timers_enabled() {
        return;
    }
    REPARSE.with(|c| {
        let (p, v, n, b) = c.get();
        c.set((p + parse, v + visit, n + 1, b + bytes as u64));
    });
}

#[inline]
pub fn record_direct_parse(parse: Duration, bytes: usize) {
    if !timers_enabled() {
        return;
    }
    REPARSE_DIRECT.with(|c| {
        let (p, n, b) = c.get();
        c.set((p + parse, n + 1, b + bytes as u64));
    });
}

pub fn take_reparse_breakdown() -> ReparseBreakdown {
    let (parse, visit, calls, bytes) = REPARSE.replace((Duration::ZERO, Duration::ZERO, 0, 0));
    let (direct_parse, direct_calls, direct_bytes) = REPARSE_DIRECT.replace((Duration::ZERO, 0, 0));
    ReparseBreakdown {
        parse,
        visit,
        calls,
        bytes,
        direct_parse,
        direct_calls,
        direct_bytes,
    }
}

#[inline]
pub fn record_esrap_client_split(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_CLIENT_SPLIT.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

#[inline]
pub fn record_esrap_client_map(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_CLIENT_MAP.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

#[inline]
pub fn record_esrap_client_plain(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_CLIENT_PLAIN.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

#[inline]
pub fn record_esrap_server(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_SERVER.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

#[inline]
pub fn record_esrap_pipe(print: Duration, reparse: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_PIPE.with(|c| {
        let (p, r, n) = c.get();
        c.set((p + print, r + reparse, n + 1));
    });
}

#[inline]
pub fn record_esrap_normalize(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ESRAP_NORMALIZE.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

pub fn take_esrap_breakdown() -> EsrapBreakdown {
    let (client_split, client_split_calls) =
        ESRAP_CLIENT_SPLIT.with(|c| c.replace((Duration::ZERO, 0)));
    let (client_map, client_map_calls) = ESRAP_CLIENT_MAP.with(|c| c.replace((Duration::ZERO, 0)));
    let (client_plain, client_plain_calls) =
        ESRAP_CLIENT_PLAIN.with(|c| c.replace((Duration::ZERO, 0)));
    let (server_print, server_print_calls) = ESRAP_SERVER.with(|c| c.replace((Duration::ZERO, 0)));
    let (server_pipe_print, server_pipe_reparse, server_pipe_calls) =
        ESRAP_PIPE.with(|c| c.replace((Duration::ZERO, Duration::ZERO, 0)));
    let (normalize_print, normalize_calls) =
        ESRAP_NORMALIZE.with(|c| c.replace((Duration::ZERO, 0)));
    EsrapBreakdown {
        client_split,
        client_split_calls,
        client_map,
        client_map_calls,
        client_plain,
        client_plain_calls,
        server_print,
        server_print_calls,
        server_pipe_print,
        server_pipe_reparse,
        server_pipe_calls,
        normalize_print,
        normalize_calls,
    }
}

#[inline]
pub fn record_visit_program(d: Duration) {
    if !timers_enabled() {
        return;
    }
    VISIT_PROGRAM.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_script_text(d: Duration) {
    if !timers_enabled() {
        return;
    }
    SCRIPT_TEXT.with(|c| c.set(c.get() + d));
    ST_PARENT_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_parent_site(is_pub: bool) {
    if !timers_enabled() {
        return;
    }
    if is_pub {
        ST_PARENT_SITE_PUB.with(|c| c.set(c.get() + 1));
    } else {
        ST_PARENT_SITE_MAIN.with(|c| c.set(c.get() + 1));
    }
}

/// Tracks how deep the staged function is on the stack, so a re-entrant call
/// is counted rather than inferred.
pub struct EntryGuard(TimerStart);

impl EntryGuard {
    #[expect(clippy::new_without_default, reason = "a guard is never defaulted")]
    pub fn new() -> Self {
        ST_DEPTH.with(|d| {
            let depth = d.get() + 1;
            d.set(depth);
            if depth > 1 {
                ST_NESTED_ENTRIES.with(|c| c.set(c.get() + 1));
            }
        });
        if ST_PARENT_OPEN.with(Cell::get) == 0 {
            ST_ENTRIES_OUTSIDE.with(|c| c.set(c.get() + 1));
        }
        Self(timer_start())
    }
}

/// Marks the span a parent timer covers, so an entry can tell whether it is
/// inside one.
pub struct ParentScope;

impl ParentScope {
    #[expect(clippy::new_without_default, reason = "a guard is never defaulted")]
    pub fn new() -> Self {
        ST_PARENT_OPEN.with(|c| c.set(c.get() + 1));
        Self
    }
}

impl Drop for ParentScope {
    fn drop(&mut self) {
        if !timers_enabled() {
            return;
        }
        ST_PARENT_OPEN.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

impl Drop for EntryGuard {
    fn drop(&mut self) {
        if !timers_enabled() {
            return;
        }
        ST_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        let elapsed = timer_elapsed(self.0);
        ST_IN_FUNCTION.with(|c| c.set(c.get() + elapsed));
    }
}

#[inline]
pub fn record_st_entry() {
    if !timers_enabled() {
        return;
    }
    ST_ENTRIES.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_template_fragment(d: Duration) {
    if !timers_enabled() {
        return;
    }
    TEMPLATE_FRAGMENT.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_assembly_after_fragment(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ASSEMBLY_AFTER_FRAGMENT.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_css_render(d: Duration) {
    if !timers_enabled() {
        return;
    }
    CSS_RENDER.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_codegen(d: Duration) {
    if !timers_enabled() {
        return;
    }
    CODEGEN.with(|c| c.set(c.get() + d));
}

/// Records into [`ScriptTextBreakdown::process_accumulated`] on drop, so the
/// statement processor's many early returns all get counted.
pub struct ProcessAccumulatedGuard(pub TimerStart);

impl Drop for ProcessAccumulatedGuard {
    fn drop(&mut self) {
        if !timers_enabled() {
            return;
        }
        ST_PROCESS_ACCUMULATED.with(|c| c.set(c.get() + timer_elapsed(self.0)));
        ST_STATEMENTS.with(|c| c.set(c.get() + 1));
    }
}

#[inline]
pub fn record_st_runes(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_RUNES.with(|c| c.set(c.get() + d));
}

/// Records the legacy `$:` branch on drop, which returns from several points.
pub struct ReactiveStmtGuard(pub TimerStart);

impl Drop for ReactiveStmtGuard {
    fn drop(&mut self) {
        if !timers_enabled() {
            return;
        }
        ST_REACTIVE_STMT.with(|c| c.set(c.get() + timer_elapsed(self.0)));
        ST_REACTIVE_CALLS.with(|c| c.set(c.get() + 1));
    }
}

#[inline]
pub fn record_st_prenormalize(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_PRENORMALIZE.with(|c| c.set(c.get() + d));
    ST_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_st_collect_vars(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_COLLECT_VARS.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_st_line_loop(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_LINE_LOOP.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_st_ast_transforms(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_AST_TRANSFORMS.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_st_post_passes(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_POST_PASSES.with(|c| c.set(c.get() + d));
}

pub fn take_script_text_breakdown() -> ScriptTextBreakdown {
    ScriptTextBreakdown {
        prenormalize: ST_PRENORMALIZE.with(|c| c.replace(Duration::ZERO)),
        collect_vars: ST_COLLECT_VARS.with(|c| c.replace(Duration::ZERO)),
        line_loop: ST_LINE_LOOP.with(|c| c.replace(Duration::ZERO)),
        ast_transforms: ST_AST_TRANSFORMS.with(|c| c.replace(Duration::ZERO)),
        post_passes: ST_POST_PASSES.with(|c| c.replace(Duration::ZERO)),
        calls: ST_CALLS.with(|c| c.replace(0)),
        process_accumulated: ST_PROCESS_ACCUMULATED.with(|c| c.replace(Duration::ZERO)),
        statements: ST_STATEMENTS.with(|c| c.replace(0)),
        runes: ST_RUNES.with(|c| c.replace(Duration::ZERO)),
        reactive_stmt: ST_REACTIVE_STMT.with(|c| c.replace(Duration::ZERO)),
        reactive_calls: ST_REACTIVE_CALLS.with(|c| c.replace(0)),
        entries: ST_ENTRIES.with(|c| c.replace(0)),
        parent_calls: ST_PARENT_CALLS.with(|c| c.replace(0)),
        nested_entries: ST_NESTED_ENTRIES.with(|c| c.replace(0)),
        parent_site_main: ST_PARENT_SITE_MAIN.with(|c| c.replace(0)),
        parent_site_pub: ST_PARENT_SITE_PUB.with(|c| c.replace(0)),
        in_function: ST_IN_FUNCTION.with(|c| c.replace(Duration::ZERO)),
        entries_outside_parent: ST_ENTRIES_OUTSIDE.with(|c| c.replace(0)),
    }
}

#[inline]
pub fn record_pipeline_parse(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_PARSE.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_line_offsets(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_LINE_OFFSETS.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_resolve_lazy(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_RESOLVE_LAZY.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_ensure_script(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_ENSURE_SCRIPT.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_ts_removal(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_TS_REMOVAL.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_options_merge(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_OPTIONS_MERGE.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_analyze(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_ANALYZE.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_transform(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_TRANSFORM.with(|c| c.set(c.get() + d));
}

/// Accumulate one analyze sub-bucket, incrementing its call count.
///
/// Written once and reused by the six recorders below so that a bucket cannot
/// be given a time without also being given a call -- the pairing is what makes
/// a zero readable, and six hand-written copies is six chances to break it.
#[inline]
fn record_analyze_bucket(cell: &'static std::thread::LocalKey<Cell<(Duration, u64)>>, d: Duration) {
    if !timers_enabled() {
        return;
    }
    cell.with(|c| {
        let (t, n) = c.get();
        c.set((t + d, n + 1));
    });
}

#[inline]
pub fn record_analyze_extract_scripts(d: Duration) {
    record_analyze_bucket(&AN_EXTRACT_SCRIPTS, d);
}

#[inline]
pub fn record_analyze_create_scopes(d: Duration) {
    record_analyze_bucket(&AN_CREATE_SCOPES, d);
}

#[inline]
pub fn record_analyze_store_subs(d: Duration) {
    record_analyze_bucket(&AN_STORE_SUBS, d);
}

#[inline]
pub fn record_analyze_template(d: Duration) {
    record_analyze_bucket(&AN_TEMPLATE, d);
}

#[inline]
pub fn record_analyze_css_analyze(d: Duration) {
    record_analyze_bucket(&AN_CSS_ANALYZE, d);
}

#[inline]
pub fn record_analyze_css_scope(d: Duration) {
    record_analyze_bucket(&AN_CSS_SCOPE, d);
}

/// One template node dispatched by `ScopeBuilder::visit_node`.
///
/// See [`AnalyzeBreakdown::create_scopes_nodes`] for what this does and does
/// not count -- it is a walk density, not a per-node denominator.
#[inline]
pub fn count_analyze_scope_node() {
    if !timers_enabled() {
        return;
    }
    AN_SCOPE_NODES.with(|c| c.set(c.get() + 1));
}

/// One template node dispatched by `visitors::visit_node`.
#[inline]
pub fn count_analyze_template_node() {
    if !timers_enabled() {
        return;
    }
    AN_TEMPLATE_NODES.with(|c| c.set(c.get() + 1));
}

/// One compile, timed end to end. Separate from the buckets on purpose: the
/// two are compared, not derived from each other.
#[inline]
pub fn record_pipeline_finalize(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_FINALIZE.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_pipeline_total(d: Duration) {
    if !timers_enabled() {
        return;
    }
    PL_TOTAL.with(|c| c.set(c.get() + d));
    PL_COMPILES.with(|c| c.set(c.get() + 1));
}

pub fn take_pipeline_breakdown() -> PipelineBreakdown {
    PipelineBreakdown {
        parse: PL_PARSE.with(|c| c.replace(Duration::ZERO)),
        line_offsets: PL_LINE_OFFSETS.with(|c| c.replace(Duration::ZERO)),
        resolve_lazy: PL_RESOLVE_LAZY.with(|c| c.replace(Duration::ZERO)),
        ensure_script: PL_ENSURE_SCRIPT.with(|c| c.replace(Duration::ZERO)),
        ts_removal: PL_TS_REMOVAL.with(|c| c.replace(Duration::ZERO)),
        options_merge: PL_OPTIONS_MERGE.with(|c| c.replace(Duration::ZERO)),
        analyze: PL_ANALYZE.with(|c| c.replace(Duration::ZERO)),
        transform: PL_TRANSFORM.with(|c| c.replace(Duration::ZERO)),
        finalize: PL_FINALIZE.with(|c| c.replace(Duration::ZERO)),
        total: PL_TOTAL.with(|c| c.replace(Duration::ZERO)),
        compiles: PL_COMPILES.with(|c| c.replace(0)),
    }
}

/// The transform bucket, read without clearing it.
///
/// [`Phase3Breakdown`] has no total of its own -- its parent is the pipeline's
/// `transform` -- so a consumer reading the sub-split needs that figure while
/// leaving it for whoever reads the pipeline split afterwards.
pub fn peek_pipeline_transform() -> Duration {
    PL_TRANSFORM.with(Cell::get)
}

/// The compile count, read without clearing it.
///
/// Same reason as [`peek_pipeline_transform`]: a consumer of the sub-split
/// needs the count that tells it whether it skipped a read, and the pipeline
/// split needs the same count afterwards.
pub fn peek_pipeline_compiles() -> u64 {
    PL_COMPILES.with(Cell::get)
}

/// The analyze sub-split: **takes** the six buckets, **peeks** the parent.
///
/// The parent (`total`) is the pipeline's `analyze` counter itself, so the two
/// are the same number by construction rather than by two timers agreeing.
/// The cost of that is an ordering constraint, the same one
/// `takePhase3Split` carries:
///
/// **Call this before [`take_pipeline_breakdown`] for the same compiles.**
/// That function clears `PL_ANALYZE`, so the other order reports `total: 0`
/// with non-zero buckets -- which a consumer checking the total catches,
/// rather than silently dividing by a zero whole.
///
/// The buckets are cleared, so calling this twice for one batch reports the
/// second read as all zeros; the call counts make that visible instead of
/// looking like work that vanished.
pub fn take_analyze_breakdown() -> AnalyzeBreakdown {
    let take = |cell: &'static std::thread::LocalKey<Cell<(Duration, u64)>>| {
        cell.with(|c| c.replace((Duration::ZERO, 0)))
    };
    let (extract_scripts, extract_scripts_calls) = take(&AN_EXTRACT_SCRIPTS);
    let (create_scopes, create_scopes_calls) = take(&AN_CREATE_SCOPES);
    let (store_subs, store_subs_calls) = take(&AN_STORE_SUBS);
    let (template, template_calls) = take(&AN_TEMPLATE);
    let (css_analyze, css_analyze_calls) = take(&AN_CSS_ANALYZE);
    let (css_scope, css_scope_calls) = take(&AN_CSS_SCOPE);
    AnalyzeBreakdown {
        extract_scripts,
        extract_scripts_calls,
        create_scopes,
        create_scopes_calls,
        store_subs,
        store_subs_calls,
        template,
        template_calls,
        create_scopes_nodes: AN_SCOPE_NODES.with(|c| c.replace(0)),
        template_nodes: AN_TEMPLATE_NODES.with(|c| c.replace(0)),
        css_analyze,
        css_analyze_calls,
        css_scope,
        css_scope_calls,
        total: PL_ANALYZE.with(Cell::get),
        compiles: PL_COMPILES.with(Cell::get),
    }
}

pub fn take_breakdown() -> Phase3Breakdown {
    Phase3Breakdown {
        visit_program: VISIT_PROGRAM.with(|c| c.replace(Duration::ZERO)),
        script_text_transform: SCRIPT_TEXT.with(|c| c.replace(Duration::ZERO)),
        template_fragment: TEMPLATE_FRAGMENT.with(|c| c.replace(Duration::ZERO)),
        assembly_after_fragment: ASSEMBLY_AFTER_FRAGMENT.with(|c| c.replace(Duration::ZERO)),
        css_render: CSS_RENDER.with(|c| c.replace(Duration::ZERO)),
        codegen: CODEGEN.with(|c| c.replace(Duration::ZERO)),
    }
}

/// Agreement between the one-pass indices and the per-variable scans they
/// replace.
///
/// The indices answer the same questions a different way, so "tests pass" is
/// not evidence they agree -- a no-op would pass too. Under
/// `RSVELTE_INDEX_ORACLE` both routes run and every answer is compared, which
/// gives the comparison a denominator instead of only a failure count.
#[derive(Default, Debug, Clone, Copy)]
pub struct IndexOracle {
    pub checks: u64,
    pub mismatches: u64,
}

thread_local! {
    static INDEX_ORACLE: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

pub fn index_oracle_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RSVELTE_INDEX_ORACLE").is_some())
}

#[inline]
pub fn record_index_oracle(agrees: bool) {
    INDEX_ORACLE.with(|c| {
        let (checks, mismatches) = c.get();
        c.set((checks + 1, mismatches + u64::from(!agrees)));
    });
}

pub fn take_index_oracle() -> IndexOracle {
    let (checks, mismatches) = INDEX_ORACLE.replace((0, 0));
    IndexOracle { checks, mismatches }
}
