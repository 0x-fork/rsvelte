//! Timers and deterministic counters for one printer call.
//!
//! The compiler already times `rsvelte_esrap` from the outside, per call site
//! (`EsrapBreakdown`: the three client branches, server, the async round-trip,
//! and normalize). Those say which entry point the cost sits behind; none of
//! them say what the cost *is*. This module splits the inside.
//!
//! # Why the counters matter more than the timers here
//!
//! The printer is 3.64x slower than the compiler it is measured against once
//! source maps are excluded from both sides -- and the two obvious explanations
//! are already ruled out by quantities measured on both compilers over the same
//! corpus: the two emit the same amount of text (1.01x) and rsvelte emits
//! *fewer* source-map segments (0.78x). So the mechanism scales with neither
//! output length nor segment count, and what is left is node count, command
//! count, and allocation count. Those are what the counters below name, and
//! being deterministic they settle in one run on a loaded machine, where the
//! timers do not.
//!
//! A fourth axis was added after the first pass: **layout decisions**. It is
//! unlike the other three, because the compiler being measured against does not
//! reproduce the reference formatter's layout at all (0 of 1296 files byte-match
//! it, averaging 23 lines shorter), and `oxc_codegen` has no layout machinery to
//! reproduce it with. So this is a quantity one side has and the other does not,
//! rather than one both have at different unit prices -- which makes it the only
//! remaining candidate that the two ruled-out quantities could not have caught.
//!
//! # Cost of the instrument
//!
//! Every counter is `O(1)` at a single call site. In particular `append_bytes`
//! sums `str.len()` per call rather than counting characters: the quantity of
//! interest is how many times the map driver's per-character loop iterates, and
//! calling `chars().count()` to learn that would run the very loop being
//! measured a second time. Bytes are an upper bound on characters and equal for
//! ASCII, which the printer's output overwhelmingly is.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Off in the shipped compiler. `rsvelte_core`'s `set_timers_enabled` drives
/// this so there is one switch rather than two that can disagree.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn the printer's instrument on or off.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[inline]
fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `None` when the gate was off at the start, so no clock is read at either end.
pub type Start = Option<std::time::Instant>;

/// Start a timer, or nothing at all when the gate is shut.
#[inline]
#[must_use]
pub fn start() -> Start {
    if enabled() {
        Some(std::time::Instant::now())
    } else {
        None
    }
}

/// Time since [`start`], or zero when the gate was shut at the start.
#[inline]
#[must_use]
pub fn elapsed(start: Start) -> Duration {
    start.map_or(Duration::ZERO, |s| s.elapsed())
}

thread_local! {
    static LINE_STARTS: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static MAP_LINE_STARTS: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static BUILD_COMMENTS: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static PRINT_PROGRAM: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static FLATTEN_MAP: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static FLATTEN_PLAIN: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };
    static RECYCLE: Cell<(Duration, u64)> = const { Cell::new((Duration::ZERO, 0)) };

    /// One `Cell` per counter rather than one `Cell<PrintCounts>`. The struct
    /// version copied all of it in and out on every increment, which at ~293
    /// increments per print was measurable in the very buckets being split:
    /// adding the layout counters moved the parent 48.3 -> 51.4 ms and inflated
    /// `print_program`'s share, because that is where most of them fire. An
    /// array indexed by [`Slot`] is one TLS lookup and one word-sized
    /// read-modify-write, and leaves the shares where they were.
    static COUNTS: [Cell<u64>; Slot::N] = const { [const { Cell::new(0) }; Slot::N] };
}

/// Index into the counter array. Order is arbitrary; only [`take`] depends on it.
#[derive(Clone, Copy)]
enum Slot {
    StmtDispatch,
    ExprDispatch,
    Contexts,
    PoolHits,
    CmdStr,
    CmdLocation,
    CmdLayout,
    CmdNested,
    MeasureReads,
    MultilineReads,
    EmptyReads,
    ReorderAppends,
    ReorderBytes,
    CrossContextAppends,
    RootlessAppends,
    ReorderTextAppends,
    ReorderTextBytes,
    StrHeap,
    StrBytes,
    AppendCalls,
    AppendBytes,
    Mappings,
}

impl Slot {
    const N: usize = Self::Mappings as usize + 1;
}

#[inline]
fn bump(slot: Slot, by: u64) {
    if !enabled() {
        return;
    }
    COUNTS.with(|c| {
        let cell = &c[slot as usize];
        cell.set(cell.get() + by);
    });
}

/// Deterministic work counts for the printer, load-independent.
#[derive(Default, Debug, Clone, Copy)]
pub struct PrintCounts {
    /// `print_statement` dispatches. One of the printer's two central `match`es,
    /// so this counts every statement it visits without touching the arms.
    pub stmt_dispatch: u64,
    /// `print_expression` dispatches, the other central `match`.
    pub expr_dispatch: u64,
    /// `Context` creations, which is also `pool::take` calls: every context
    /// takes exactly one buffer. Counted rather than read off `pool.rs`'s
    /// doc comment, which says "nearly every node" -- there are eight
    /// `child()` sites, sitting in per-item and per-statement loops, and the
    /// real number is a measurement, not a phrase.
    pub contexts: u64,
    /// Buffers `pool::take` handed back from the free list rather than
    /// allocating. `contexts - pool_hits` is how many `Vec`s were allocated.
    pub pool_hits: u64,
    /// `Command`s pushed, by variant. `Str` and `Location` carry payload; the
    /// rest are one-byte layout sentinels, so they are counted together.
    pub cmd_str: u64,
    /// `Command::Location`s pushed.
    pub cmd_location: u64,
    /// Whitespace and indent sentinels pushed.
    pub cmd_layout: u64,
    /// `Command::Nested`s pushed, which is also `Context::append` calls: every
    /// append splices exactly one child buffer. The fourth variant, and the one
    /// the first pass of this instrument left out of "total commands".
    pub cmd_nested: u64,
    /// Reads of `Context::measure`, `Context::multiline` and `Context::empty` --
    /// the three inputs to every layout decision the printer makes, and the
    /// count of layout decisions themselves up to how many inputs each reads.
    ///
    /// Counted in the accessors rather than at the decision sites: `multiline`
    /// was a public field, and making it private with a getter turns "did I
    /// enumerate every branch" into "there is one way to ask", which stays true
    /// when branches are added. The same reason the two dispatch counters sit on
    /// the two central `match`es.
    pub measure_reads: u64,
    /// Reads of `Context::multiline`; see `measure_reads`.
    pub multiline_reads: u64,
    /// Reads of `Context::empty`; see `measure_reads`.
    pub empty_reads: u64,
    /// `Context::append`s where the parent pushed commands between building the
    /// child and splicing it — so the parent holds text belonging *before* text
    /// the child already produced.
    ///
    /// This is the count that says whether the command tree is still load
    /// bearing. Writing straight into one buffer reaches the same output only
    /// when every append is in build order; each of these is a place where a
    /// single buffer would have to move bytes it had already written.
    pub reorder_appends: u64,
    /// Literal bytes inside the children of `reorder_appends` — how much text a
    /// single buffer would have had to move, rather than how often.
    pub reorder_bytes: u64,
    /// Appends where the parent held *fewer* commands than when the child was
    /// born. Impossible while a child is spliced into the context that made it,
    /// since a command buffer only grows, so this is the witness that
    /// `reorder_appends` compares two lengths from the same buffer. A non-zero
    /// value invalidates it.
    pub cross_context_appends: u64,
    /// The subset of `reorder_appends` where the text pushed into the gap was
    /// itself text, rather than only sentinels or a `Location`. This is the
    /// count a single buffer would actually have to move bytes for; the wider
    /// count charges it for whitespace it could have resolved for free.
    pub reorder_text_appends: u64,
    /// Literal bytes inside the children of `reorder_text_appends`.
    pub reorder_text_bytes: u64,
    /// Appends of a context built by `Context::new` rather than `Context::child`,
    /// which carry no birth length and so are outside `reorder_appends`'
    /// denominator. Reported rather than assumed zero.
    pub rootless_appends: u64,
    /// `Command::Str` payloads that did not fit `CompactString`'s inline
    /// storage. Read from `is_heap_allocated()` rather than comparing against
    /// the 24-byte limit named in a doc comment.
    pub str_heap: u64,
    /// Bytes handed to `Command::Str`, the size of the text the tree carries.
    pub str_bytes: u64,
    /// `Driver::append` calls and the bytes they were given, in the source-map
    /// driver only. `append_bytes` bounds the per-character loop's iteration
    /// count from above (equal for ASCII).
    pub append_calls: u64,
    /// Bytes handed to `Driver::append`; see `append_calls`.
    pub append_bytes: u64,
    /// `Mapping`s pushed, one per `Command::Location` the map driver consumes.
    pub mappings: u64,
}

/// Timers for the printer's five serial steps, plus the counts.
///
/// The steps are `lib.rs`'s own sequence, so the residual against an enclosing
/// timer is whatever the entry function does around them -- expected to be
/// small, and reported rather than assumed.
#[derive(Default, Debug, Clone, Copy)]
pub struct PrintBreakdown {
    /// `line_starts` over the program's own source.
    pub line_starts: Duration,
    /// Calls to it.
    pub line_starts_calls: u64,
    /// `line_starts` over the source-map source, in `print_split` only. Kept
    /// apart from the one above because this one exists *because of* source
    /// maps, so it belongs to the map cost rather than to printing.
    pub map_line_starts: Duration,
    /// Calls to it.
    pub map_line_starts_calls: u64,
    /// Time resolving the program's comments to offsets and bodies.
    pub build_comments: Duration,
    /// Calls to it.
    pub build_comments_calls: u64,
    /// The AST walk that builds the command tree.
    pub print_program: Duration,
    /// Calls to it.
    pub print_program_calls: u64,
    /// Flattening the tree into text, source-map driver.
    pub flatten_map: Duration,
    /// Calls to it.
    pub flatten_map_calls: u64,
    /// Flattening the tree into text, plain driver.
    pub flatten_plain: Duration,
    /// Calls to it.
    pub flatten_plain_calls: u64,
    /// Time returning the tree's buffers to the pool.
    pub recycle: Duration,
    /// Calls to it.
    pub recycle_calls: u64,
    /// Deterministic work counts for the same calls.
    pub counts: PrintCounts,
}

impl PrintBreakdown {
    /// The five steps' time, for checking against an enclosing timer.
    pub fn total(&self) -> Duration {
        self.line_starts
            + self.map_line_starts
            + self.build_comments
            + self.print_program
            + self.flatten_map
            + self.flatten_plain
            + self.recycle
    }
}

macro_rules! recorder {
    ($(#[$doc:meta])* $name:ident, $cell:ident) => {
        $(#[$doc])*
        #[inline]
        pub fn $name(d: Duration) {
            if !enabled() {
                return;
            }
            $cell.with(|c| {
                let (t, n) = c.get();
                c.set((t + d, n + 1));
            });
        }
    };
}

recorder!(
    /// Time in `line_starts` over the program's own source.
    record_line_starts,
    LINE_STARTS
);
recorder!(
    /// Time in `line_starts` over the source-map source. Source-map cost, not
    /// printing cost, which is why it is not folded into the one above.
    record_map_line_starts,
    MAP_LINE_STARTS
);
recorder!(
    /// Time in `build_comments`.
    record_build_comments,
    BUILD_COMMENTS
);
recorder!(
    /// Time in the AST walk that builds the command tree.
    record_print_program,
    PRINT_PROGRAM
);
recorder!(
    /// Time flattening the tree, source-map driver.
    record_flatten_map,
    FLATTEN_MAP
);
recorder!(
    /// Time flattening the tree, plain driver.
    record_flatten_plain,
    FLATTEN_PLAIN
);
recorder!(
    /// Time returning the tree's buffers to the pool.
    record_recycle,
    RECYCLE
);

macro_rules! counter {
    ($(#[$doc:meta])* $name:ident, $slot:ident) => {
        $(#[$doc])*
        #[inline]
        pub fn $name() {
            bump(Slot::$slot, 1);
        }
    };
}

counter!(
    /// One `print_statement` dispatch.
    count_stmt_dispatch,
    StmtDispatch
);
counter!(
    /// One `print_expression` dispatch.
    count_expr_dispatch,
    ExprDispatch
);
counter!(
    /// One `Command::Location` pushed onto a context.
    count_cmd_location,
    CmdLocation
);
counter!(
    /// One whitespace/indent sentinel pushed onto a context.
    count_cmd_layout,
    CmdLayout
);
counter!(
    /// One `Command::Nested` pushed, i.e. one `Context::append`.
    count_cmd_nested,
    CmdNested
);
counter!(
    /// One read of `Context::measure`.
    count_measure_read,
    MeasureReads
);
counter!(
    /// One read of `Context::multiline`.
    count_multiline_read,
    MultilineReads
);
counter!(
    /// One read of `Context::empty`.
    count_empty_read,
    EmptyReads
);

/// One `Context` creation, and whether the pool had a buffer for it.
#[inline]
pub fn count_context(pool_hit: bool) {
    bump(Slot::Contexts, 1);
    bump(Slot::PoolHits, u64::from(pool_hit));
}

/// One `Command::Str`: its size, and whether the payload spilled to the heap.
#[inline]
pub fn count_cmd_str(bytes: usize, heap: bool) {
    bump(Slot::CmdStr, 1);
    bump(Slot::StrBytes, bytes as u64);
    bump(Slot::StrHeap, u64::from(heap));
}

/// One `Driver::append`, with the byte length it was handed. See the module
/// docs for why this is bytes and not characters.
#[inline]
pub fn count_append(bytes: usize) {
    bump(Slot::AppendCalls, 1);
    bump(Slot::AppendBytes, bytes as u64);
}

/// One `Context::append`, told whether the child could still have been written
/// straight into its parent's buffer.
///
/// `spans` is `None` for a child with no recorded birth, and otherwise the
/// parent's command count when the child was born and again at the splice.
#[inline]
pub fn count_append_order(
    spans: Option<(usize, usize)>,
    text_spans: Option<(usize, usize)>,
    child_bytes: usize,
) {
    let Some((born, now)) = spans else {
        bump(Slot::RootlessAppends, 1);
        return;
    };
    if now > born {
        bump(Slot::ReorderAppends, 1);
        bump(Slot::ReorderBytes, child_bytes as u64);
    } else if now < born {
        bump(Slot::CrossContextAppends, 1);
    }
    if let Some((text_born, text_now)) = text_spans
        && text_now > text_born
    {
        bump(Slot::ReorderTextAppends, 1);
        bump(Slot::ReorderTextBytes, child_bytes as u64);
    }
}

/// One `Mapping` pushed.
#[inline]
pub fn count_mapping() {
    bump(Slot::Mappings, 1);
}

/// Read the breakdown and clear it. Calling twice for one batch reports the
/// second read as zeros; the `*_calls` make that visible rather than looking
/// like work that vanished.
pub fn take() -> PrintBreakdown {
    let take_pair = |cell: &'static std::thread::LocalKey<Cell<(Duration, u64)>>| {
        cell.with(|c| c.replace((Duration::ZERO, 0)))
    };
    let (line_starts, line_starts_calls) = take_pair(&LINE_STARTS);
    let (map_line_starts, map_line_starts_calls) = take_pair(&MAP_LINE_STARTS);
    let (build_comments, build_comments_calls) = take_pair(&BUILD_COMMENTS);
    let (print_program, print_program_calls) = take_pair(&PRINT_PROGRAM);
    let (flatten_map, flatten_map_calls) = take_pair(&FLATTEN_MAP);
    let (flatten_plain, flatten_plain_calls) = take_pair(&FLATTEN_PLAIN);
    let (recycle, recycle_calls) = take_pair(&RECYCLE);
    PrintBreakdown {
        line_starts,
        line_starts_calls,
        map_line_starts,
        map_line_starts_calls,
        build_comments,
        build_comments_calls,
        print_program,
        print_program_calls,
        flatten_map,
        flatten_map_calls,
        flatten_plain,
        flatten_plain_calls,
        recycle,
        recycle_calls,
        counts: COUNTS.with(|c| {
            let get = |slot: Slot| c[slot as usize].replace(0);
            PrintCounts {
                stmt_dispatch: get(Slot::StmtDispatch),
                expr_dispatch: get(Slot::ExprDispatch),
                contexts: get(Slot::Contexts),
                pool_hits: get(Slot::PoolHits),
                cmd_str: get(Slot::CmdStr),
                cmd_location: get(Slot::CmdLocation),
                cmd_layout: get(Slot::CmdLayout),
                cmd_nested: get(Slot::CmdNested),
                measure_reads: get(Slot::MeasureReads),
                multiline_reads: get(Slot::MultilineReads),
                empty_reads: get(Slot::EmptyReads),
                reorder_appends: get(Slot::ReorderAppends),
                reorder_bytes: get(Slot::ReorderBytes),
                cross_context_appends: get(Slot::CrossContextAppends),
                rootless_appends: get(Slot::RootlessAppends),
                reorder_text_appends: get(Slot::ReorderTextAppends),
                reorder_text_bytes: get(Slot::ReorderTextBytes),
                str_heap: get(Slot::StrHeap),
                str_bytes: get(Slot::StrBytes),
                append_calls: get(Slot::AppendCalls),
                append_bytes: get(Slot::AppendBytes),
                mappings: get(Slot::Mappings),
            }
        }),
    }
}
