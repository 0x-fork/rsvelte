//! Prices the one operation a flat output buffer would have to pay that the
//! command tree does not.
//!
//! # Why this exists
//!
//! `esrap_split` established that the printer's command tree is no longer needed
//! for layout decisions — `measure`, `empty` and `multiline` are scalars kept
//! current by `write`/`append`, where upstream esrap re-walks the command array.
//! What the tree still buys is *ordering*: a parent that writes text between
//! building a child and splicing it holds output belonging before output the
//! child has already produced. The tree makes that splice `O(1)`; a single
//! contiguous buffer would have to move the child's bytes.
//!
//! That is the only entry in the four-way sort that lands under "replacing it
//! makes things *worse*", and it is the one term left unmeasured. Its size
//! decides the question: at 96.3 reorders per print, a 10 ns insertion costs
//! ~1 µs and a 50 ns one costs ~4.8 µs, which is the difference between "second
//! target, worth recording" and "does not survive the payoff sieve".
//!
//! # What is measured
//!
//! `String::insert_str` at a position leaving `tail` bytes after it — reserve,
//! `memmove` the tail up, copy the new bytes in. The corpus says the tail is
//! 47.6 bytes on average (5,920,785 bytes across 124,273 reordering appends on
//! flowbite 1296), so that size is the one that matters; the others are here to
//! show the shape, because a single point cannot distinguish a per-call constant
//! from a per-byte slope and the two extrapolate differently.
//!
//! This is a microbenchmark and is reported as one: it prices an operation in
//! isolation, on a buffer that stays hot, with no surrounding printer work. It
//! is a *lower* bound on what the real thing would cost, which is the useful
//! direction — the term it feeds is the one that argues *against* flattening, so
//! understating it is the conservative error.
//!
//! ```text
//! cargo run --release -p rsvelte_devtools --bin insert_cost
//! ```

use std::hint::black_box;
use std::time::Instant;

/// Output bytes one print produces on flowbite 1296, so the buffer being
/// inserted into is the size the real one would be.
const BUFFER_BYTES: usize = 2610;

/// What a reordering parent writes: `"? "`, `" : "`, a separator. Small.
const INSERTED: &str = "? ";

/// Insertions per timed batch. Large enough that the clock read is negligible,
/// small enough that the buffer has not grown far past `BUFFER_BYTES`.
const BATCH: usize = 512;

fn time_insert(tail: usize, batches: usize) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..batches {
        // Rebuilt per batch so every batch starts at the same length, and the
        // growth from BATCH insertions never compounds across batches.
        let mut buffer = "x".repeat(BUFFER_BYTES);
        let start = Instant::now();
        for _ in 0..BATCH {
            let at = buffer.len() - tail;
            buffer.insert_str(at, INSERTED);
            black_box(&buffer);
        }
        let elapsed = start.elapsed().as_secs_f64() * 1e9 / BATCH as f64;
        best = best.min(elapsed);
    }
    best
}

fn main() {
    // Named rather than hard-coded: the four-way sort quotes a per-print tree
    // size derived from it, and a layout change would silently move it.
    let command_size = rsvelte_esrap::profile::command_size();
    println!("size_of::<Command>() = {command_size}");
    println!(
        "1086.9 commands per print x {command_size} B = {:.1} KB of tree per print",
        1086.9 * command_size as f64 / 1024.0
    );
    println!(
        "\nString::insert_str of {} bytes into a {} byte buffer",
        INSERTED.len(),
        BUFFER_BYTES
    );
    println!("min of {} batches x {} insertions each\n", 64, BATCH);
    println!("{:>10}{:>14}", "tail bytes", "ns each");
    for tail in [0, 16, 32, 48, 64, 128, 256, 512, 1024] {
        println!("{tail:>10}{:>14.2}", time_insert(tail, 64));
    }

    // The number the sort actually needs, stated with its inputs so it can be
    // rederived rather than trusted.
    let per_print = 96.3;
    let mean_tail = 48;
    let each = time_insert(mean_tail, 64);
    println!(
        "\nmeasured constant {:.2} ns at the corpus mean tail ({} bytes)",
        each, mean_tail
    );
    println!(
        "x {per_print} reorders per print = {:.2} us/print",
        each * per_print / 1000.0
    );
}
