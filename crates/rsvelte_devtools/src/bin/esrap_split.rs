//! Sub-split of the `rsvelte_esrap` printer, taken from inside the production
//! pipeline.
//!
//! The existing `EsrapBreakdown` splits the printer **by call site** — the three
//! client branches, server, the async round-trip, normalize. That says which
//! entry point the cost sits behind. This one splits the **inside** of a call
//! into the five serial steps `lib.rs` performs, and counts the work each did.
//!
//! # What question this answers
//!
//! Measured against svelte-rs on the same corpus in the same run, the printer is
//! 2.76x slower with source maps on both sides, and **3.64x** with them off —
//! removing maps makes rsvelte look worse, not better. So the excess is in plain
//! printing, and two candidate explanations are already dead: the two compilers
//! emit the same amount of text (1.01x) and rsvelte emits *fewer* source-map
//! segments (0.78x). Whatever is left scales with neither output length nor
//! segment count.
//!
//! The counters below name the remaining axes — node count, command count,
//! allocation count, and layout decisions. They are deterministic, so unlike the
//! timers beside them they settle in one run on a loaded machine.
//!
//! The last axis is the odd one out, and worth the extra column: the compiler
//! being compared against reproduces the reference formatter's layout in 0 of
//! 1296 files (averaging 23 lines shorter), and `oxc_codegen` carries no layout
//! machinery to reproduce it with. So it is a quantity one side has and the
//! other structurally does not — which is exactly the shape a mechanism has to
//! have to survive the two ruled-out quantities above.
//!
//! # Parent
//!
//! `EsrapBreakdown`'s six call-site buckets summed. The five steps should
//! account for nearly all of it; whatever they do not is the entry functions'
//! own work, and it is printed rather than folded into a neighbour.
//!
//! ```text
//! cargo run --release -p rsvelte_devtools --bin esrap_split -- <dir>...
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use rsvelte_core::compiler::compile_with_external_sourcemap_content;
use rsvelte_core::compiler::phases::phase3_transform::profile;
use rsvelte_core::{CompileOptions, GenerateMode};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "svelte") {
            out.push(path);
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    profile::set_timers_enabled(true);
    let mut roots: Vec<PathBuf> = Vec::new();
    // `--plain` sends every print through the non-map driver. That arm is not
    // about source maps: it is the only way to price a *first* traversal of the
    // command tree without the per-character loop on top. `recycle` also walks
    // the whole tree cheaply, but it runs after `flatten` has already pulled the
    // tree through cache, so it cannot answer whether this layout is expensive
    // to walk cold. `flatten_plain` can.
    let mut plain = false;
    for arg in std::env::args().skip(1) {
        if arg == "--plain" {
            plain = true;
        } else {
            roots.push(PathBuf::from(arg));
        }
    }
    if roots.is_empty() {
        eprintln!("usage: esrap_split [--plain] <dir>...");
        std::process::exit(1);
    }

    let mut files = Vec::new();
    for root in &roots {
        collect(root, &mut files);
    }
    files.sort();
    let sources: Vec<String> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    if sources.is_empty() {
        eprintln!("no .svelte files under the given roots");
        std::process::exit(1);
    }

    for source in sources.iter().take(100) {
        let _ = compile_with_external_sourcemap_content(source, CompileOptions::default());
    }
    // Both readers, or the warm-up's counts join the timed pass.
    let _ = rsvelte_esrap::profile::take();
    let _ = profile::take_esrap_breakdown();

    for source in &sources {
        let _ = compile_with_external_sourcemap_content(
            source,
            CompileOptions {
                generate: GenerateMode::Client,
                dev: false,
                enable_sourcemap: !plain,
                ..Default::default()
            },
        );
    }

    let inner = rsvelte_esrap::profile::take();
    let outer = profile::take_esrap_breakdown();

    let parent = outer.client_split
        + outer.client_map
        + outer.client_plain
        + outer.server_print
        + outer.server_pipe_print
        + outer.normalize_print;
    let parent_ms = ms(parent);
    let pct = |d: Duration| {
        if parent_ms == 0.0 {
            f64::NAN
        } else {
            ms(d) / parent_ms * 100.0
        }
    };

    // Components with no script never reach esrap (a hand-written printer
    // handles them), so `files` is not the denominator for a per-print figure.
    // The print calls are.
    let prints = outer.client_split_calls
        + outer.client_map_calls
        + outer.client_plain_calls
        + outer.server_print_calls
        + outer.server_pipe_calls
        + outer.normalize_calls;
    println!(
        "arm: {}",
        if plain {
            "--plain (non-map driver, no per-character loop)"
        } else {
            "default (source maps on, as the compiler ships)"
        }
    );
    println!(
        "{} files, {} print calls ({:.1}% of files reached esrap)",
        sources.len(),
        prints,
        prints as f64 / sources.len() as f64 * 100.0
    );

    println!("\n-- call sites (existing instrument, the parent) --");
    println!("{:<20}{:>12}{:>10}{:>10}", "site", "ms", "share", "calls");
    for (name, d, calls) in [
        ("client_split", outer.client_split, outer.client_split_calls),
        ("client_map", outer.client_map, outer.client_map_calls),
        ("client_plain", outer.client_plain, outer.client_plain_calls),
        ("server_print", outer.server_print, outer.server_print_calls),
        ("server_pipe", outer.server_pipe_print, outer.server_pipe_calls),
        ("normalize", outer.normalize_print, outer.normalize_calls),
    ] {
        println!("{name:<20}{:>12.1}{:>9.1}%{calls:>10}", ms(d), pct(d));
    }

    println!("\n-- inside one print (the five serial steps) --");
    println!("{:<20}{:>12}{:>10}{:>10}", "step", "ms", "share", "calls");
    for (name, d, calls) in [
        ("line_starts", inner.line_starts, inner.line_starts_calls),
        (
            "map_line_starts",
            inner.map_line_starts,
            inner.map_line_starts_calls,
        ),
        (
            "build_comments",
            inner.build_comments,
            inner.build_comments_calls,
        ),
        (
            "print_program",
            inner.print_program,
            inner.print_program_calls,
        ),
        ("flatten_map", inner.flatten_map, inner.flatten_map_calls),
        (
            "flatten_plain",
            inner.flatten_plain,
            inner.flatten_plain_calls,
        ),
        ("recycle", inner.recycle, inner.recycle_calls),
    ] {
        println!("{name:<20}{:>12.1}{:>9.1}%{calls:>10}", ms(d), pct(d));
    }
    let unattributed = parent.saturating_sub(inner.total());
    println!(
        "{:<20}{:>12.1}{:>9.1}%{:>10}",
        "unattributed",
        ms(unattributed),
        pct(unattributed),
        ""
    );
    println!(
        "{:<20}{parent_ms:>12.1}{:>9.1}%{:>10}",
        "parent (call sites)", 100.0, ""
    );

    // The two structural checks, printed every run rather than assumed: the
    // steps must be entered exactly once per print, and the timers must agree
    // with the call sites that enclose them.
    let step_calls = inner.line_starts_calls;
    println!(
        "\ncalls check: line_starts {} vs print calls {} -> {}",
        step_calls,
        prints,
        if step_calls == prints { "ok" } else { "MISMATCH" }
    );
    println!(
        "flatten check: map {} + plain {} = {} vs print calls {} -> {}",
        inner.flatten_map_calls,
        inner.flatten_plain_calls,
        inner.flatten_map_calls + inner.flatten_plain_calls,
        prints,
        if inner.flatten_map_calls + inner.flatten_plain_calls == prints {
            "ok"
        } else {
            "MISMATCH"
        }
    );

    let c = inner.counts;
    let per = |n: u64| n as f64 / prints.max(1) as f64;
    // All four variants. The first pass of this reader summed three and called
    // it "total"; `Nested` is a command like any other -- pushed by the builder,
    // matched by both drivers, and freed by `recycle`.
    let commands = c.cmd_str + c.cmd_location + c.cmd_layout + c.cmd_nested;
    let nodes = c.stmt_dispatch + c.expr_dispatch;
    let decisions = c.measure_reads + c.multiline_reads + c.empty_reads;
    println!("\n-- work counts (deterministic, load-independent) --");
    println!(
        "{:<24}{:>14}{:>14}",
        "quantity", "total", "per print"
    );
    for (name, n) in [
        ("stmt dispatches", c.stmt_dispatch),
        ("expr dispatches", c.expr_dispatch),
        ("nodes (stmt+expr)", nodes),
        ("contexts created", c.contexts),
        ("  of which pooled", c.pool_hits),
        ("  of which allocated", c.contexts - c.pool_hits),
        ("commands: Str", c.cmd_str),
        ("commands: Location", c.cmd_location),
        ("commands: layout", c.cmd_layout),
        ("commands: Nested", c.cmd_nested),
        ("commands: total", commands),
        ("  Str payload bytes", c.str_bytes),
        ("  Str heap-allocated", c.str_heap),
        ("append calls (map only)", c.append_calls),
        ("append bytes (map only)", c.append_bytes),
        ("mappings pushed", c.mappings),
        ("layout: measure reads", c.measure_reads),
        ("layout: multiline reads", c.multiline_reads),
        ("layout: empty reads", c.empty_reads),
        ("layout: decision inputs", decisions),
        ("appends out of order", c.reorder_appends),
        ("  their bytes", c.reorder_bytes),
        ("  of those, text moved", c.reorder_text_appends),
        ("  its bytes", c.reorder_text_bytes),
        ("appends w/o birth", c.rootless_appends),
        ("appends cross-context", c.cross_context_appends),
    ] {
        println!("{name:<24}{n:>14}{:>14.1}", per(n));
    }

    // The question the tree exists to answer: could the printer have written
    // straight into one buffer? Only if every child is spliced in build order.
    // `cross_context` must be zero for the comparison to be between two lengths
    // of the same buffer, so it is checked rather than assumed.
    let ordered = c
        .cmd_nested
        .saturating_sub(c.reorder_appends)
        .saturating_sub(c.rootless_appends);
    println!(
        "\nappends in build order {} / {} ({:.1}%); out of order {} carrying {} bytes",
        ordered,
        c.cmd_nested,
        ordered as f64 / c.cmd_nested.max(1) as f64 * 100.0,
        c.reorder_appends,
        c.reorder_bytes
    );
    println!(
        "of those, the gap held text {} ({:.1}% of all appends) carrying {} bytes",
        c.reorder_text_appends,
        c.reorder_text_appends as f64 / c.cmd_nested.max(1) as f64 * 100.0,
        c.reorder_text_bytes
    );
    println!(
        "cross-context check: {} -> {}",
        c.cross_context_appends,
        if c.cross_context_appends == 0 {
            "ok (both lengths come from the same buffer)"
        } else {
            "MISMATCH (reorder_appends is not meaningful)"
        }
    );

    // Counts are not time. Dividing the step that produced or consumed a
    // quantity by that quantity gives the unit price the numbers imply, and a
    // price that is implausible for the operation is the counter's own check:
    // too high and work nobody counted is sitting in the bucket, too low and the
    // counter is counting more than the bucket does.
    println!("\n-- implied unit price (bucket time / its count) --");
    println!("{:<40}{:>12}", "operation", "ns each");
    let ns = |d: Duration, n: u64| {
        if n == 0 {
            f64::NAN
        } else {
            d.as_secs_f64() * 1e9 / n as f64
        }
    };
    for (name, d, n) in [
        (
            "print_program / command pushed",
            inner.print_program,
            commands,
        ),
        ("print_program / node dispatched", inner.print_program, nodes),
        ("flatten_map / command read", inner.flatten_map, commands),
        (
            "flatten_plain / command read",
            inner.flatten_plain,
            commands,
        ),
        (
            "flatten_map / output byte",
            inner.flatten_map,
            c.append_bytes,
        ),
        ("recycle / buffer returned", inner.recycle, c.contexts),
    ] {
        println!("{name:<40}{:>12.2}", ns(d, n));
    }

    // `cmd_location` counts what the printer emitted; `mappings` counts what the
    // map driver consumed. They differ by the Locations built on prints that
    // then flattened without maps -- work produced and thrown away, which is
    // invisible in any single number.
    println!(
        "\nLocations emitted {} vs mappings consumed {} -> {} built and discarded",
        c.cmd_location,
        c.mappings,
        c.cmd_location.saturating_sub(c.mappings)
    );
}
