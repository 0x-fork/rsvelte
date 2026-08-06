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
    /// Text-only prologue inside [`Self::process_accumulated`].
    pub pa_prologue: Duration,
    pub pa_prologue_calls: u64,
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
    static AT_PROBE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_PARSE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_WALK: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_OUTPUT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_STORE_UNSUB: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_PARSE_CALLS: Cell<u64> = const { Cell::new(0) };
    static AT_WALK_CALLS: Cell<u64> = const { Cell::new(0) };
    static TF_DEPTH: Cell<u32> = const { Cell::new(0) };
    static TF_CLEAN: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static TF_TEMPLATE_STR: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static TF_AS_JSON: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static TF_PARSE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static TF_CLEAN_CALLS: Cell<u64> = const { Cell::new(0) };
    static TF_TEMPLATE_STR_CALLS: Cell<u64> = const { Cell::new(0) };
    static TF_AS_JSON_CALLS: Cell<u64> = const { Cell::new(0) };
    static TF_PARSE_CALLS: Cell<u64> = const { Cell::new(0) };
    static AS_DEPTH: Cell<u32> = const { Cell::new(0) };
    static AS_MODULE_TEXT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AS_CSS_INJECT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AS_AS_JSON: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AS_PARSE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AS_MODULE_TEXT_CALLS: Cell<u64> = const { Cell::new(0) };
    static AS_CSS_INJECT_CALLS: Cell<u64> = const { Cell::new(0) };
    static AS_AS_JSON_CALLS: Cell<u64> = const { Cell::new(0) };
    static AS_PARSE_CALLS: Cell<u64> = const { Cell::new(0) };
    static RS_SETUP: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static RS_SHADOW_FIX: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static RS_ASYNC_PRE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static RS_WARNINGS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static RS_SOURCEMAP: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static RS_SETUP_CALLS: Cell<u64> = const { Cell::new(0) };
    static RS_SHADOW_FIX_CALLS: Cell<u64> = const { Cell::new(0) };
    static RS_ASYNC_PRE_CALLS: Cell<u64> = const { Cell::new(0) };
    static RS_WARNINGS_CALLS: Cell<u64> = const { Cell::new(0) };
    static RS_SOURCEMAP_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCAN_BYTES: Cell<u64> = const { Cell::new(0) };
    static SCAN_CALLS: Cell<u64> = const { Cell::new(0) };
    static SCAN_SCRIPT_BYTES: Cell<u64> = const { Cell::new(0) };
    static CV_ANALYSIS_VECS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CV_TEXT_INDEX: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CV_BINDING_VECS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CV_SET_MAPS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_PA_PROLOGUE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static ST_PA_PROLOGUE_CALLS: Cell<u64> = const { Cell::new(0) };
    static TEXT_CHECKED: Cell<u64> = const { Cell::new(0) };
    static TEXT_CHANGED: Cell<u64> = const { Cell::new(0) };
    static TEXT_UNEXPLAINED: Cell<u64> = const { Cell::new(0) };
    static TEXT_NOOP: Cell<u64> = const { Cell::new(0) };
    static CV_PROXY_VARS: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CV_LINE_SPLIT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static CV_CALLS: Cell<[u64; 6]> = const { Cell::new([0; 6]) };
    static REWRITE_CALLS: Cell<[u64; REWRITE_SITE_COUNT]> = const { Cell::new([0; REWRITE_SITE_COUNT]) };
    static REWRITE_FILES: Cell<[u64; REWRITE_SITE_COUNT]> = const { Cell::new([0; REWRITE_SITE_COUNT]) };
    static REWRITE_SEEN: Cell<[bool; REWRITE_SITE_COUNT]> = const { Cell::new([false; REWRITE_SITE_COUNT]) };
    static SCAN_SITE_BYTES: Cell<[u64; SCAN_SITE_COUNT]> = const { Cell::new([0; SCAN_SITE_COUNT]) };
    static SCAN_SITE_CALLS: Cell<[u64; SCAN_SITE_COUNT]> = const { Cell::new([0; SCAN_SITE_COUNT]) };
    static AT_REACTIVE_APPEND: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_RETAINED_GATE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_CANDIDATE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static AT_PROBE_CTX: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static LS_STAGED: Cell<u64> = const { Cell::new(0) };
    static LS_GATED: Cell<u64> = const { Cell::new(0) };
    static LS_MATCHED: Cell<u64> = const { Cell::new(0) };
    static LS_MISMATCHED: Cell<u64> = const { Cell::new(0) };
    static LS_GROUPS_SCAN: Cell<u64> = const { Cell::new(0) };
    static LS_GROUPS_AST: Cell<u64> = const { Cell::new(0) };
    static PP_SHADOW: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PP_PROP_MUTATION: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static PP_DEV_TAIL: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static DP_TIME: Cell<[Duration; DP_SITE_COUNT]> =
        const { Cell::new([Duration::ZERO; DP_SITE_COUNT]) };
    static DP_CALLS: Cell<[u64; DP_SITE_COUNT]> = const { Cell::new([0; DP_SITE_COUNT]) };
    static DP_BYTES: Cell<[u64; DP_SITE_COUNT]> = const { Cell::new([0; DP_SITE_COUNT]) };
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
    record_tf_parse(parse);
}

#[inline]
pub fn record_direct_parse(site: usize, parse: Duration, bytes: usize) {
    if !timers_enabled() {
        return;
    }
    REPARSE_DIRECT.with(|c| {
        let (p, n, b) = c.get();
        c.set((p + parse, n + 1, b + bytes as u64));
    });
    DP_TIME.with(|c| {
        let mut v = c.get();
        v[site] += parse;
        c.set(v);
    });
    DP_CALLS.with(|c| {
        let mut v = c.get();
        v[site] += 1;
        c.set(v);
    });
    DP_BYTES.with(|c| {
        let mut v = c.get();
        v[site] += bytes as u64;
        c.set(v);
    });
    record_tf_parse(parse);
}

/// Attributes a parse to the fragment visitor when one is open around it.
///
/// Hooked onto the existing recorders rather than added at the parse sites, so
/// the bucket cannot miss a site the sites themselves already cover.
#[inline]
fn record_tf_parse(d: Duration) {
    if in_tf_scope() {
        TF_PARSE.with(|c| c.set(c.get() + d));
        TF_PARSE_CALLS.with(|c| c.set(c.get() + 1));
    } else if in_as_scope() {
        AS_PARSE.with(|c| c.set(c.get() + d));
        AS_PARSE_CALLS.with(|c| c.set(c.get() + 1));
    }
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

/// The statement processor's text-only prologue: rejoining the accumulated
/// lines, then dispatching on `starts_with("export function ")` and friends and
/// rebuilding the string without the `export ` keyword.
///
/// Timed apart from the transforms that follow it because an AST path is handed
/// the node and its kind, so none of this work has a counterpart there -- while
/// the transforms after it still have to happen in some form.
#[inline]
pub fn record_st_pa_prologue(d: Duration) {
    if !timers_enabled() {
        return;
    }
    ST_PA_PROLOGUE.with(|c| c.set(c.get() + d));
    ST_PA_PROLOGUE_CALLS.with(|c| c.set(c.get() + 1));
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

/// Bytes handed to a text scan inside the instance-script pipeline, against the
/// script's own length.
///
/// `bytes / script_bytes` is the effective number of times the pipeline walks
/// its input -- a load-independent integer, which is what a 15x ratio needs
/// rather than a share table.
///
/// **Upper bound.** A scan that finds its needle stops there, so charging the
/// whole haystack over-counts; a scan that fails does read all of it. The bound
/// is in the useful direction: it cannot make the excess look smaller than it is.
#[derive(Default, Debug, Clone, Copy)]
pub struct ScanCounts {
    pub bytes: u64,
    pub calls: u64,
    pub script_bytes: u64,
    pub site_bytes: [u64; SCAN_SITE_COUNT],
    pub site_calls: [u64; SCAN_SITE_COUNT],
}

/// Scan sites, split far enough to tell "the staged text pipeline" from the one
/// post pass that rebuilds a whole-script index after every rewrite.
pub const SCAN_SITE_STAGED: usize = 0;
pub const SCAN_SITE_SHADOW_INDEX: usize = 1;
pub const SCAN_SITE_SHADOW_ENCLOSING: usize = 2;
pub const SCAN_SITE_SHADOW_BODY: usize = 3;
/// The `collect_vars` extractors. Separate from `staged` because the same two
/// functions also run on the module script, outside the staged pipeline, so
/// folding them in would put out-of-scope work in a bucket the size bands read.
pub const SCAN_SITE_CV_TEXT: usize = 4;
/// `extract_proxy_vars` sits in a different sub-timer than the other extractor,
/// so it gets its own site: a single figure spanning two timers cannot be
/// divided into either of them.
pub const SCAN_SITE_CV_PROXY: usize = 5;
pub const SCAN_SITE_COUNT: usize = 6;
pub const SCAN_SITE_NAMES: [&str; SCAN_SITE_COUNT] = [
    "staged",
    "shadow_index",
    "shadow_enclosing",
    "shadow_body",
    "cv_text (local_reactive)",
    "cv_proxy (proxy_vars)",
];

/// Where the staged script pipeline first stops being a reader.
///
/// The question these answer is not "how expensive is this rewrite" but "is the
/// Phase 2 AST still valid here": every site below replaces the script text, so
/// the spans the parser produced stop lining up from that point on. A file that
/// trips none of them reaches the line loop byte-identical to what was parsed.
pub const REWRITE_PN_COMMENTS: usize = 0;
pub const REWRITE_PN_CLASS_FIELDS: usize = 1;
pub const REWRITE_PN_SPLIT_DECLS: usize = 2;
pub const REWRITE_PN_ARROW_PARENS: usize = 3;
/// Set by every site above, so files that trip two of them are counted once.
pub const REWRITE_PN_ANY: usize = 4;
pub const REWRITE_SITE_COUNT: usize = 5;
pub const REWRITE_SITE_NAMES: [&str; REWRITE_SITE_COUNT] = [
    "pn_strip_comments",
    "pn_class_fields",
    "pn_split_decls",
    "pn_arrow_parens",
    "pn_ANY (union)",
];

#[derive(Default, Debug, Clone, Copy)]
pub struct RewriteCounts {
    pub calls: [u64; REWRITE_SITE_COUNT],
    pub files: [u64; REWRITE_SITE_COUNT],
}

/// The passes that build their own `Parser` instead of going through the shared
/// driver, one constant per call site rather than per module.
///
/// Two sites in the same file can differ in the only property that matters here
/// -- whether they re-parse a whole script or a single wrapped expression -- so
/// folding them by module would average a 10-byte parse into a 3000-byte one and
/// report a size that neither site has.
pub const DP_AST_STATE: usize = 0;
pub const DP_NORMALIZE: usize = 1;
pub const DP_PRIVATE_CLASS: usize = 2;
/// The retry that wraps the source in a synthetic class after the first parse
/// reported diagnostics. Separate from [`DP_PRIVATE_CLASS`] because it only
/// fires on the failure path: merged, a rare double parse would look like a
/// common single one.
pub const DP_PRIVATE_CLASS_WRAPPED: usize = 3;
pub const DP_SEMANTIC: usize = 4;
pub const DP_PROPS_IS_SIMPLE: usize = 5;
pub const DP_PROPS_SHOULD_PROXY: usize = 6;
pub const DP_MODULE_COMMENTS: usize = 7;
pub const DP_DESTR_IS_SIMPLE: usize = 8;
pub const DP_DESTR_LITERAL_KEY: usize = 9;
pub const DP_SITE_COUNT: usize = 10;
pub const DP_SITE_NAMES: [&str; DP_SITE_COUNT] = [
    "ast_state (instance script)",
    "normalize (generated JS)",
    "private_class",
    "private_class (wrapped retry)",
    "with_semantic",
    "props ast_expr_is_simple",
    "props ast_should_proxy",
    "strip_module_toplevel_comments",
    "destructure is_simple",
    "destructure literal_key",
];

/// Per-site split of [`ReparseBreakdown::direct_parse`].
///
/// Reported against the unsplit totals rather than on its own: a site array can
/// be short by a whole call site and still look plausible, and the only thing
/// that catches that is `Σ sites == direct_calls` printed every run.
#[derive(Debug, Clone, Copy)]
pub struct DirectParseSites {
    pub time: [Duration; DP_SITE_COUNT],
    pub calls: [u64; DP_SITE_COUNT],
    pub bytes: [u64; DP_SITE_COUNT],
}

impl Default for DirectParseSites {
    fn default() -> Self {
        Self {
            time: [Duration::ZERO; DP_SITE_COUNT],
            calls: [0; DP_SITE_COUNT],
            bytes: [0; DP_SITE_COUNT],
        }
    }
}

pub fn take_direct_parse_sites() -> DirectParseSites {
    DirectParseSites {
        time: DP_TIME.with(|c| c.replace([Duration::ZERO; DP_SITE_COUNT])),
        calls: DP_CALLS.with(|c| c.replace([0; DP_SITE_COUNT])),
        bytes: DP_BYTES.with(|c| c.replace([0; DP_SITE_COUNT])),
    }
}

/// Blinds one rewrite site, so the text-identity gate's `unexplained` counter
/// can be shown to move. A gate whose failure column is structurally unable to
/// leave zero reports the same thing whether or not it works.
pub fn gate_selftest_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RSVELTE_GATE_SELFTEST").is_some())
}

#[inline]
pub fn count_rewrite(site: usize) {
    if !timers_enabled() {
        return;
    }
    if gate_selftest_enabled() && site == REWRITE_PN_ARROW_PARENS {
        return;
    }
    if site != REWRITE_PN_ANY {
        count_rewrite(REWRITE_PN_ANY);
    }
    REWRITE_CALLS.with(|c| {
        let mut v = c.get();
        v[site] += 1;
        c.set(v);
    });
    REWRITE_SEEN.with(|c| {
        let mut v = c.get();
        if !v[site] {
            v[site] = true;
            c.set(v);
            REWRITE_FILES.with(|f| {
                let mut fv = f.get();
                fv[site] += 1;
                f.set(fv);
            });
        }
    });
}

/// Clears the per-file "already counted" flags so `files` counts files, not calls.
#[inline]
pub fn rewrite_file_boundary() {
    if !timers_enabled() {
        return;
    }
    REWRITE_SEEN.with(|c| c.set([false; REWRITE_SITE_COUNT]));
}

pub fn take_rewrite_counts() -> RewriteCounts {
    RewriteCounts {
        calls: REWRITE_CALLS.with(|c| c.replace([0; REWRITE_SITE_COUNT])),
        files: REWRITE_FILES.with(|c| c.replace([0; REWRITE_SITE_COUNT])),
    }
}

pub fn take_scan_counts() -> ScanCounts {
    ScanCounts {
        bytes: SCAN_BYTES.with(|c| c.replace(0)),
        calls: SCAN_CALLS.with(|c| c.replace(0)),
        script_bytes: SCAN_SCRIPT_BYTES.with(|c| c.replace(0)),
        site_bytes: SCAN_SITE_BYTES.with(|c| c.replace([0; SCAN_SITE_COUNT])),
        site_calls: SCAN_SITE_CALLS.with(|c| c.replace([0; SCAN_SITE_COUNT])),
    }
}

#[inline]
pub fn count_scan(bytes: usize) {
    count_scan_site(SCAN_SITE_STAGED, bytes);
}

#[inline]
pub fn count_scan_site(site: usize, bytes: usize) {
    if !timers_enabled() {
        return;
    }
    SCAN_BYTES.with(|c| c.set(c.get() + bytes as u64));
    SCAN_CALLS.with(|c| c.set(c.get() + 1));
    SCAN_SITE_BYTES.with(|c| {
        let mut v = c.get();
        v[site] += bytes as u64;
        c.set(v);
    });
    SCAN_SITE_CALLS.with(|c| {
        let mut v = c.get();
        v[site] += 1;
        c.set(v);
    });
}

#[inline]
pub fn count_script_len(bytes: usize) {
    if !timers_enabled() {
        return;
    }
    SCAN_SCRIPT_BYTES.with(|c| c.set(c.get() + bytes as u64));
}

/// The Phase 3 residual, i.e. `transform` minus the six named buckets.
///
/// `analyze` taught the shape of this question: its residual was the largest
/// bucket by both time and work, and splitting it found not "cost with no
/// name" but a walk nobody had named. These four are the regions of the client
/// transform and its wrapper that no timer covered.
#[derive(Default, Debug, Clone, Copy)]
pub struct ResidualBreakdown {
    /// Transform options, state and context construction, before `visit_program`.
    pub setup: Duration,
    /// The shadowed-`$state` transform cleanup between `visit_program` and the
    /// script-text stage.
    pub shadow_fix: Duration,
    /// The async blocker-map precompute between the script and the fragment.
    pub async_pre: Duration,
    /// Warning conversion and the unused-CSS-selector scan.
    pub warnings: Duration,
    /// Source-map assembly in the transform wrapper.
    pub sourcemap: Duration,
    pub setup_calls: u64,
    pub shadow_fix_calls: u64,
    pub async_pre_calls: u64,
    pub warnings_calls: u64,
    pub sourcemap_calls: u64,
}

pub fn take_residual_breakdown() -> ResidualBreakdown {
    ResidualBreakdown {
        setup: RS_SETUP.with(|c| c.replace(Duration::ZERO)),
        shadow_fix: RS_SHADOW_FIX.with(|c| c.replace(Duration::ZERO)),
        async_pre: RS_ASYNC_PRE.with(|c| c.replace(Duration::ZERO)),
        warnings: RS_WARNINGS.with(|c| c.replace(Duration::ZERO)),
        sourcemap: RS_SOURCEMAP.with(|c| c.replace(Duration::ZERO)),
        setup_calls: RS_SETUP_CALLS.with(|c| c.replace(0)),
        shadow_fix_calls: RS_SHADOW_FIX_CALLS.with(|c| c.replace(0)),
        async_pre_calls: RS_ASYNC_PRE_CALLS.with(|c| c.replace(0)),
        warnings_calls: RS_WARNINGS_CALLS.with(|c| c.replace(0)),
        sourcemap_calls: RS_SOURCEMAP_CALLS.with(|c| c.replace(0)),
    }
}

macro_rules! residual_recorder {
    ($name:ident, $cell:ident, $calls:ident) => {
        #[inline]
        pub fn $name(d: Duration) {
            if !timers_enabled() {
                return;
            }
            $cell.with(|c| c.set(c.get() + d));
            $calls.with(|c| c.set(c.get() + 1));
        }
    };
}

residual_recorder!(record_rs_setup, RS_SETUP, RS_SETUP_CALLS);
residual_recorder!(record_rs_shadow_fix, RS_SHADOW_FIX, RS_SHADOW_FIX_CALLS);
residual_recorder!(record_rs_async_pre, RS_ASYNC_PRE, RS_ASYNC_PRE_CALLS);
residual_recorder!(record_rs_warnings, RS_WARNINGS, RS_WARNINGS_CALLS);
residual_recorder!(record_rs_sourcemap, RS_SOURCEMAP, RS_SOURCEMAP_CALLS);

/// One level below `ScriptTextBreakdown::collect_vars`.
///
/// This stage turned out not to be text scanning at all, so the split is by
/// *what the work is made of* rather than by what an AST would remove: three
/// shapes of `Vec<String>` / set / map construction over Phase 2 bindings, the
/// one genuinely text-driven index, and the line split that feeds the loop.
#[derive(Default, Debug, Clone, Copy)]
pub struct CollectVarsBreakdown {
    /// `state_vars` / `var_state_vars` off the root scope's declarations.
    pub analysis_vecs: Duration,
    /// `extract_local_reactive_vars` + the two one-pass text indexes and the
    /// classification loop over them -- the only text-driven part.
    pub text_index: Duration,
    /// `extract_proxy_vars`: one line walk plus a find per line. Timed apart
    /// from the binding builds it sits between, because a scan and a `Vec`
    /// build answer different questions about what to do with the bucket.
    pub proxy_vars: Duration,
    /// The long run of `bindings.iter().filter().map(clone).collect()` builds.
    pub binding_vecs: Duration,
    /// Hash set/map builds (`name_occurrences`, `names_all_non_proxy`, ...).
    pub set_maps: Duration,
    /// `script_rest.lines().collect()` and the depth-tracking state init.
    pub line_split: Duration,
    /// One per sub-timer, so a mismatch localises to a boundary instead of
    /// only showing up as a stage-level discrepancy.
    pub calls: [u64; 6],
}

/// Whether the staged pipeline's text is still the text Phase 2 parsed.
///
/// Reading the code says which statements *can* rewrite `script_rest`; only
/// hashing the string at both ends says whether any of them, or anything the
/// reading missed, actually did. Compare `changed` against the union of the
/// rewrite-site counters: a higher count means a rewrite path nobody has named.
#[derive(Default, Debug, Clone, Copy)]
pub struct TextIdentity {
    pub checked: u64,
    pub changed: u64,
    /// Files whose text changed although no named rewrite site fired. This, not
    /// `changed` against a count, is the test for an unnamed rewrite path: a
    /// count comparison can be satisfied by one unknown path cancelling out
    /// several named sites that happened to be no-ops.
    pub unexplained: u64,
    /// Files where a named site fired and the text came out identical anyway.
    pub noop: u64,
}

pub fn take_text_identity() -> TextIdentity {
    TextIdentity {
        checked: TEXT_CHECKED.with(|c| c.replace(0)),
        changed: TEXT_CHANGED.with(|c| c.replace(0)),
        unexplained: TEXT_UNEXPLAINED.with(|c| c.replace(0)),
        noop: TEXT_NOOP.with(|c| c.replace(0)),
    }
}

#[inline]
pub fn record_text_identity(changed: bool) {
    if !timers_enabled() {
        return;
    }
    let named_fired = REWRITE_SEEN.with(|c| c.get()[REWRITE_PN_ANY]);
    TEXT_CHECKED.with(|c| c.set(c.get() + 1));
    if changed {
        TEXT_CHANGED.with(|c| c.set(c.get() + 1));
        if !named_fired {
            TEXT_UNEXPLAINED.with(|c| c.set(c.get() + 1));
        }
    } else if named_fired {
        TEXT_NOOP.with(|c| c.set(c.get() + 1));
    }
}

pub fn take_collect_vars_breakdown() -> CollectVarsBreakdown {
    CollectVarsBreakdown {
        analysis_vecs: CV_ANALYSIS_VECS.with(|c| c.replace(Duration::ZERO)),
        text_index: CV_TEXT_INDEX.with(|c| c.replace(Duration::ZERO)),
        proxy_vars: CV_PROXY_VARS.with(|c| c.replace(Duration::ZERO)),
        binding_vecs: CV_BINDING_VECS.with(|c| c.replace(Duration::ZERO)),
        set_maps: CV_SET_MAPS.with(|c| c.replace(Duration::ZERO)),
        line_split: CV_LINE_SPLIT.with(|c| c.replace(Duration::ZERO)),
        calls: CV_CALLS.with(|c| c.replace([0; 6])),
    }
}

macro_rules! cv_recorder {
    ($name:ident, $cell:ident, $idx:expr) => {
        #[inline]
        pub fn $name(d: Duration) {
            if !timers_enabled() {
                return;
            }
            $cell.with(|c| c.set(c.get() + d));
            CV_CALLS.with(|c| {
                let mut v = c.get();
                v[$idx] += 1;
                c.set(v);
            });
        }
    };
}

cv_recorder!(record_cv_analysis_vecs, CV_ANALYSIS_VECS, 0);
cv_recorder!(record_cv_text_index, CV_TEXT_INDEX, 1);
cv_recorder!(record_cv_proxy_vars, CV_PROXY_VARS, 2);
cv_recorder!(record_cv_binding_vecs, CV_BINDING_VECS, 3);
cv_recorder!(record_cv_set_maps, CV_SET_MAPS, 4);
cv_recorder!(record_cv_line_split, CV_LINE_SPLIT, 5);

/// One level below [`Phase3Breakdown::assembly_after_fragment`].
///
/// Same three-way question as [`TemplateFragmentBreakdown`]: only a parse, a
/// splice, or a string built to be re-parsed can go away if the stage is fed an
/// AST. `css_inject` is here to be *excluded* from that sum -- the stylesheet
/// text is what the compiler emits, so it is output, not representation.
#[derive(Default, Debug, Clone, Copy)]
pub struct AssemblyBreakdown {
    /// The `<script module>` text pipeline: import extraction, class-field and
    /// rune rewrites, top-level comment stripping.
    pub module_text: Duration,
    /// `render_stylesheet` for `css="injected"`. Output, not representation.
    pub css_inject: Duration,
    /// `as_json` materialisation reached from inside the assembly stage.
    pub as_json: Duration,
    /// oxc parses reached from inside the assembly stage.
    pub parse: Duration,
    pub module_text_calls: u64,
    pub css_inject_calls: u64,
    pub as_json_calls: u64,
    pub parse_calls: u64,
}

pub fn take_assembly_breakdown() -> AssemblyBreakdown {
    AssemblyBreakdown {
        module_text: AS_MODULE_TEXT.with(|c| c.replace(Duration::ZERO)),
        css_inject: AS_CSS_INJECT.with(|c| c.replace(Duration::ZERO)),
        as_json: AS_AS_JSON.with(|c| c.replace(Duration::ZERO)),
        parse: AS_PARSE.with(|c| c.replace(Duration::ZERO)),
        module_text_calls: AS_MODULE_TEXT_CALLS.with(|c| c.replace(0)),
        css_inject_calls: AS_CSS_INJECT_CALLS.with(|c| c.replace(0)),
        as_json_calls: AS_AS_JSON_CALLS.with(|c| c.replace(0)),
        parse_calls: AS_PARSE_CALLS.with(|c| c.replace(0)),
    }
}

#[inline]
pub fn as_scope_enter() {
    if !timers_enabled() {
        return;
    }
    AS_DEPTH.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn as_scope_exit() {
    if !timers_enabled() {
        return;
    }
    AS_DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
}

#[inline]
fn in_as_scope() -> bool {
    AS_DEPTH.with(|c| c.get()) > 0
}

#[inline]
pub fn record_as_module_text(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AS_MODULE_TEXT.with(|c| c.set(c.get() + d));
    AS_MODULE_TEXT_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_as_css_inject(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AS_CSS_INJECT.with(|c| c.set(c.get() + d));
    AS_CSS_INJECT_CALLS.with(|c| c.set(c.get() + 1));
}

/// One level below [`Phase3Breakdown::template_fragment`].
///
/// The buckets are the three mechanisms that produce or consume text or a
/// serialised form -- a parse, the HTML template string, and the `as_json`
/// materialisation -- plus `clean_nodes`, which scans the template's own text.
/// Everything else is the visitor building the js_ast IR and is left in the
/// residual, because that is the work no change of representation removes.
///
/// `text splice` has no bucket on purpose: a directory-wide grep for
/// `replace_range` across `3_transform/` finds every site in the *script*
/// pipeline and none under the template visitor, so the bucket would be a
/// structural zero rather than a measurement.
#[derive(Default, Debug, Clone, Copy)]
pub struct TemplateFragmentBreakdown {
    /// `clean_nodes`: whitespace trimming and node organisation.
    pub clean: Duration,
    /// `transform_template`: assembling the `$.from_html("…")` string.
    pub template_str: Duration,
    /// `as_json` materialisation reached from inside the fragment visitor.
    pub as_json: Duration,
    /// oxc parses reached from inside the fragment visitor.
    pub parse: Duration,
    pub clean_calls: u64,
    pub template_str_calls: u64,
    pub as_json_calls: u64,
    pub parse_calls: u64,
}

pub fn take_template_fragment_breakdown() -> TemplateFragmentBreakdown {
    TemplateFragmentBreakdown {
        clean: TF_CLEAN.with(|c| c.replace(Duration::ZERO)),
        template_str: TF_TEMPLATE_STR.with(|c| c.replace(Duration::ZERO)),
        as_json: TF_AS_JSON.with(|c| c.replace(Duration::ZERO)),
        parse: TF_PARSE.with(|c| c.replace(Duration::ZERO)),
        clean_calls: TF_CLEAN_CALLS.with(|c| c.replace(0)),
        template_str_calls: TF_TEMPLATE_STR_CALLS.with(|c| c.replace(0)),
        as_json_calls: TF_AS_JSON_CALLS.with(|c| c.replace(0)),
        parse_calls: TF_PARSE_CALLS.with(|c| c.replace(0)),
    }
}

/// Opens the scope the buckets above are attributed to.
///
/// Depth rather than a flag because the fragment visitor recurses; the buckets
/// accumulate leaf mechanisms, so only "are we anywhere inside" matters.
#[inline]
pub fn tf_scope_enter() {
    if !timers_enabled() {
        return;
    }
    TF_DEPTH.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn tf_scope_exit() {
    if !timers_enabled() {
        return;
    }
    TF_DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
}

#[inline]
fn in_tf_scope() -> bool {
    TF_DEPTH.with(|c| c.get()) > 0
}

#[inline]
pub fn record_tf_clean(d: Duration) {
    if !timers_enabled() || !in_tf_scope() {
        return;
    }
    TF_CLEAN.with(|c| c.set(c.get() + d));
    TF_CLEAN_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_tf_template_str(d: Duration) {
    if !timers_enabled() || !in_tf_scope() {
        return;
    }
    TF_TEMPLATE_STR.with(|c| c.set(c.get() + d));
    TF_TEMPLATE_STR_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_tf_as_json(d: Duration) {
    if !timers_enabled() {
        return;
    }
    if in_tf_scope() {
        TF_AS_JSON.with(|c| c.set(c.get() + d));
        TF_AS_JSON_CALLS.with(|c| c.set(c.get() + 1));
    } else if in_as_scope() {
        AS_AS_JSON.with(|c| c.set(c.get() + d));
        AS_AS_JSON_CALLS.with(|c| c.set(c.get() + 1));
    }
}

/// One level below [`ScriptTextBreakdown::ast_transforms`].
///
/// Split to answer whether feeding this stage an AST instead of the text
/// pipeline's output would pay: only `parse` and `output` can go away, because
/// `parse` exists solely to recover a program from text and `output` is the
/// splice back into text. `walk` is the rewrite itself and survives any change
/// of input. There is no printer to bucket -- this stage never serialises an
/// AST, it edits the source string in place -- so a parse/walk/print split
/// would report a structural zero for the third column.
#[derive(Default, Debug, Clone, Copy)]
pub struct AstTransformsBreakdown {
    /// Rune probes, local-state collection and derived-name gathering, all of
    /// which read the script text before any AST work starts.
    pub probe: Duration,
    /// `Parser::parse` on the text pipeline's output, i.e. the reparse the
    /// retained program is meant to avoid.
    pub parse: Duration,
    /// Replacement collection, `SemanticBuilder` included.
    pub walk: Duration,
    /// Sorting the replacements and splicing them into a fresh `String`.
    pub output: Duration,
    /// `wrap_store_unsub_for_state_sets`, a text scan that runs after the AST
    /// transform and is neither parse nor walk.
    pub store_unsub: Duration,
    pub parse_calls: u64,
    pub walk_calls: u64,
    /// Sorting the pending `$:` statements and concatenating them onto the
    /// result. Text assembly, with no counterpart once the pass owns nodes.
    pub reactive_append: Duration,
    /// Deciding whether the retained program still describes the text: two
    /// `trim`s and a whole-script `!=`, plus the projection attempt when the
    /// comparison passes. Charged whether or not the fast path is taken, which
    /// is the point -- it is what the text detour costs even on the files the
    /// retained program cannot help.
    pub retained_gate: Duration,
    /// `has_state_transform_candidate`, the text probe that gates the reparse.
    pub candidate: Duration,
    /// The second `record_at_probe` site: building the config struct rather
    /// than scanning. Split out because only the first site is text work, and a
    /// single `probe` figure cannot be sorted into (a) or (b).
    pub probe_ctx: Duration,
}

/// Agreement between the line scan's statement boundaries and the ones the
/// Phase 2 program implies.
///
/// Whether the scan can be replaced by the parser's answer is a measurement,
/// not a reading of the code: the scan has hand-written rules for cases the
/// grammar settles differently, and the only way to know they agree is to run
/// both and compare. `staged` is the denominator, `gated` the files where the
/// retained program still describes the text, `matched` the ones where the two
/// enumerations are identical.
#[derive(Default, Debug, Clone, Copy)]
pub struct LineSplitAgreement {
    pub staged: u64,
    pub gated: u64,
    pub matched: u64,
    /// Files that passed the gate but produced a different group sequence, kept
    /// apart from `gated - matched` so a counting slip cannot hide inside a
    /// subtraction.
    pub mismatched: u64,
    pub groups_scan: u64,
    pub groups_ast: u64,
}

pub fn line_split_dual_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RSVELTE_LINE_SPLIT_DUAL").is_some())
}

#[inline]
pub fn record_line_split(gated: bool, matched: bool, groups_scan: usize, groups_ast: usize) {
    if !timers_enabled() {
        return;
    }
    LS_STAGED.with(|c| c.set(c.get() + 1));
    if !gated {
        return;
    }
    LS_GATED.with(|c| c.set(c.get() + 1));
    if matched {
        LS_MATCHED.with(|c| c.set(c.get() + 1));
    } else {
        LS_MISMATCHED.with(|c| c.set(c.get() + 1));
    }
    LS_GROUPS_SCAN.with(|c| c.set(c.get() + groups_scan as u64));
    LS_GROUPS_AST.with(|c| c.set(c.get() + groups_ast as u64));
}

pub fn take_line_split_agreement() -> LineSplitAgreement {
    LineSplitAgreement {
        staged: LS_STAGED.with(|c| c.replace(0)),
        gated: LS_GATED.with(|c| c.replace(0)),
        matched: LS_MATCHED.with(|c| c.replace(0)),
        mismatched: LS_MISMATCHED.with(|c| c.replace(0)),
        groups_scan: LS_GROUPS_SCAN.with(|c| c.replace(0)),
        groups_ast: LS_GROUPS_AST.with(|c| c.replace(0)),
    }
}

/// The passes after the AST transform, split far enough to say which of them
/// still reads the whole script as text.
#[derive(Default, Debug, Clone, Copy)]
pub struct PostPassesBreakdown {
    pub shadow: Duration,
    pub prop_mutation: Duration,
    pub dev_tail: Duration,
}

pub fn take_post_passes_breakdown() -> PostPassesBreakdown {
    PostPassesBreakdown {
        shadow: PP_SHADOW.with(|c| c.replace(Duration::ZERO)),
        prop_mutation: PP_PROP_MUTATION.with(|c| c.replace(Duration::ZERO)),
        dev_tail: PP_DEV_TAIL.with(|c| c.replace(Duration::ZERO)),
    }
}

#[inline]
pub fn record_pp_shadow(d: Duration) {
    if timers_enabled() {
        PP_SHADOW.with(|c| c.set(c.get() + d));
    }
}

#[inline]
pub fn record_pp_prop_mutation(d: Duration) {
    if timers_enabled() {
        PP_PROP_MUTATION.with(|c| c.set(c.get() + d));
    }
}

#[inline]
pub fn record_pp_dev_tail(d: Duration) {
    if timers_enabled() {
        PP_DEV_TAIL.with(|c| c.set(c.get() + d));
    }
}

#[inline]
pub fn record_at_reactive_append(d: Duration) {
    if timers_enabled() {
        AT_REACTIVE_APPEND.with(|c| c.set(c.get() + d));
    }
}

#[inline]
pub fn record_at_retained_gate(d: Duration) {
    if timers_enabled() {
        AT_RETAINED_GATE.with(|c| c.set(c.get() + d));
    }
}

#[inline]
pub fn record_at_candidate(d: Duration) {
    if timers_enabled() {
        AT_CANDIDATE.with(|c| c.set(c.get() + d));
    }
}

/// Records the config-building probe site into both the existing `probe` total
/// and its own bucket, so the split is additive against a figure taken before
/// the split existed.
#[inline]
pub fn record_at_probe_ctx(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_PROBE.with(|c| c.set(c.get() + d));
    AT_PROBE_CTX.with(|c| c.set(c.get() + d));
}

pub fn take_ast_transforms_breakdown() -> AstTransformsBreakdown {
    AstTransformsBreakdown {
        probe: AT_PROBE.with(|c| c.replace(Duration::ZERO)),
        parse: AT_PARSE.with(|c| c.replace(Duration::ZERO)),
        walk: AT_WALK.with(|c| c.replace(Duration::ZERO)),
        output: AT_OUTPUT.with(|c| c.replace(Duration::ZERO)),
        store_unsub: AT_STORE_UNSUB.with(|c| c.replace(Duration::ZERO)),
        parse_calls: AT_PARSE_CALLS.with(|c| c.replace(0)),
        walk_calls: AT_WALK_CALLS.with(|c| c.replace(0)),
        reactive_append: AT_REACTIVE_APPEND.with(|c| c.replace(Duration::ZERO)),
        retained_gate: AT_RETAINED_GATE.with(|c| c.replace(Duration::ZERO)),
        candidate: AT_CANDIDATE.with(|c| c.replace(Duration::ZERO)),
        probe_ctx: AT_PROBE_CTX.with(|c| c.replace(Duration::ZERO)),
    }
}

#[inline]
pub fn record_at_probe(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_PROBE.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_at_parse(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_PARSE.with(|c| c.set(c.get() + d));
    AT_PARSE_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_at_walk(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_WALK.with(|c| c.set(c.get() + d));
    AT_WALK_CALLS.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn record_at_output(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_OUTPUT.with(|c| c.set(c.get() + d));
}

#[inline]
pub fn record_at_store_unsub(d: Duration) {
    if !timers_enabled() {
        return;
    }
    AT_STORE_UNSUB.with(|c| c.set(c.get() + d));
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
        pa_prologue: ST_PA_PROLOGUE.with(|c| c.replace(Duration::ZERO)),
        pa_prologue_calls: ST_PA_PROLOGUE_CALLS.with(|c| c.replace(0)),
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
