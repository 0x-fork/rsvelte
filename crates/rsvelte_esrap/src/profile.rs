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

    static COUNTS: Cell<PrintCounts> = const { Cell::new(PrintCounts::ZERO) };
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

impl PrintCounts {
    const ZERO: Self = Self {
        stmt_dispatch: 0,
        expr_dispatch: 0,
        contexts: 0,
        pool_hits: 0,
        cmd_str: 0,
        cmd_location: 0,
        cmd_layout: 0,
        str_heap: 0,
        str_bytes: 0,
        append_calls: 0,
        append_bytes: 0,
        mappings: 0,
    };
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
    ($(#[$doc:meta])* $name:ident, $field:ident) => {
        $(#[$doc])*
        #[inline]
        pub fn $name() {
            if !enabled() {
                return;
            }
            COUNTS.with(|c| {
                let mut v = c.get();
                v.$field += 1;
                c.set(v);
            });
        }
    };
}

counter!(
    /// One `print_statement` dispatch.
    count_stmt_dispatch,
    stmt_dispatch
);
counter!(
    /// One `print_expression` dispatch.
    count_expr_dispatch,
    expr_dispatch
);
counter!(
    /// One `Command::Location` pushed onto a context.
    count_cmd_location,
    cmd_location
);
counter!(
    /// One whitespace/indent sentinel pushed onto a context.
    count_cmd_layout,
    cmd_layout
);

/// One `Context` creation, and whether the pool had a buffer for it.
#[inline]
pub fn count_context(pool_hit: bool) {
    if !enabled() {
        return;
    }
    COUNTS.with(|c| {
        let mut v = c.get();
        v.contexts += 1;
        v.pool_hits += u64::from(pool_hit);
        c.set(v);
    });
}

/// One `Command::Str`: its size, and whether the payload spilled to the heap.
#[inline]
pub fn count_cmd_str(bytes: usize, heap: bool) {
    if !enabled() {
        return;
    }
    COUNTS.with(|c| {
        let mut v = c.get();
        v.cmd_str += 1;
        v.str_bytes += bytes as u64;
        v.str_heap += u64::from(heap);
        c.set(v);
    });
}

/// One `Driver::append`, with the byte length it was handed. See the module
/// docs for why this is bytes and not characters.
#[inline]
pub fn count_append(bytes: usize) {
    if !enabled() {
        return;
    }
    COUNTS.with(|c| {
        let mut v = c.get();
        v.append_calls += 1;
        v.append_bytes += bytes as u64;
        c.set(v);
    });
}

/// One `Mapping` pushed.
#[inline]
pub fn count_mapping() {
    if !enabled() {
        return;
    }
    COUNTS.with(|c| {
        let mut v = c.get();
        v.mappings += 1;
        c.set(v);
    });
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
        counts: COUNTS.with(|c| c.replace(PrintCounts::ZERO)),
    }
}
