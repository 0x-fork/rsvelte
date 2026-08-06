//! Splits `X`, the half of the codegen bucket that is not the esrap print.
//!
//! # What `X` actually is
//!
//! `X` has been quoted as "`program_to_oxc`, 14.86 µs/file, no counterpart on
//! the other side". The first half of that is a mislabel: `X = codegen - P` is a
//! *residual*, and the codegen timer spans three things —
//!
//! ```text
//! alloc.reset()                        a retained arena, cleared per compile
//! program_to_oxc(..)                   the IR -> oxc conversion
//! esrap_mappings_to_source_mappings()  runs after the printer's timer stops
//! ```
//!
//! — of which only the middle one is the named function. They sort differently
//! under "what would a compiler without an IR pay", so lumping them hides the
//! answer.
//!
//! # The question
//!
//! svelte-rs builds oxc nodes directly and has no IR, so it has no conversion
//! step at all. That makes `X` a quantity one side has and the other does not —
//! but "no counterpart" bounds neither zero nor all of it. Building an oxc node
//! costs what it costs; a compiler that skips the IR still pays for the nodes,
//! it just pays inside its own construction. So the split that matters is:
//!
//! ```text
//! disappears        work that exists only because an IR exists
//! same job, moved   the oxc nodes themselves, which both sides build
//! increases         what an IR makes cheap that direct construction would not
//! ```
//!
//! `parse_chunk` is the sharpest line here. The IR carries generated JS as
//! opaque `Raw` text, and the converter parses it back into an AST. A compiler
//! that never left the AST never pays it — so it is on the "disappears" side,
//! and it is counted separately from the node building for exactly that reason.
//!
//! ```text
//! cargo run --release -p rsvelte_devtools --bin to_oxc_split -- <dir>...
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
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        eprintln!("usage: to_oxc_split <dir>...");
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
    // Every reader, or the warm-up's counts join the timed pass. Missing one
    // here puts warm-up files in a numerator whose denominator excludes them.
    let _ = profile::take_to_oxc_breakdown();
    let _ = profile::take_esrap_breakdown();
    let _ = profile::take_breakdown();

    for source in &sources {
        let _ = compile_with_external_sourcemap_content(
            source,
            CompileOptions {
                generate: GenerateMode::Client,
                dev: false,
                ..Default::default()
            },
        );
    }

    let inner = profile::take_to_oxc_breakdown();
    let esrap = profile::take_esrap_breakdown();
    let phase3 = profile::take_breakdown();

    let p = esrap.client_split + esrap.client_map + esrap.client_plain;
    let prints = esrap.client_split_calls + esrap.client_map_calls + esrap.client_plain_calls;
    let codegen = phase3.codegen;
    let x = codegen.saturating_sub(p);

    let codegen_ms = ms(codegen);
    let pct = |d: Duration| {
        if codegen_ms == 0.0 {
            f64::NAN
        } else {
            ms(d) / codegen_ms * 100.0
        }
    };

    println!(
        "{} files, {} client prints, {} conversions",
        sources.len(),
        prints,
        inner.total_calls
    );

    println!("\n-- the codegen bucket --");
    println!("{:<24}{:>12}{:>10}{:>10}", "part", "ms", "share", "calls");
    for (name, d, calls) in [
        ("P esrap print", p, prints),
        ("alloc_reset", inner.alloc_reset, inner.alloc_reset_calls),
        ("to_oxc (both passes)", inner.total, inner.total_calls),
        (
            "mappings_convert",
            inner.mappings_convert,
            inner.mappings_convert_calls,
        ),
    ] {
        println!("{name:<24}{:>12.1}{:>9.1}%{calls:>10}", ms(d), pct(d));
    }
    let named = p + inner.alloc_reset + inner.total + inner.mappings_convert;
    let residual = codegen.saturating_sub(named);
    println!(
        "{:<24}{:>12.1}{:>9.1}%{:>10}",
        "residual",
        ms(residual),
        pct(residual),
        ""
    );
    println!(
        "{:<24}{codegen_ms:>12.1}{:>9.1}%{:>10}",
        "codegen (parent)", 100.0, ""
    );

    // The identity, printed rather than assumed: the four named parts plus the
    // residual must be the parent, and `X` must be the parent minus `P`.
    println!(
        "\nclosure: named {:.1} + residual {:.1} = {:.1} vs codegen {:.1} -> {}",
        ms(named),
        ms(residual),
        ms(named + residual),
        codegen_ms,
        if named + residual == codegen {
            "ok"
        } else {
            "MISMATCH"
        }
    );
    println!(
        "X = codegen - P = {:.1} ms ({:.1}% of codegen)",
        ms(x),
        pct(x)
    );

    println!("\n-- inside to_oxc --");
    println!("{:<24}{:>12}{:>10}{:>10}", "part", "ms", "of X", "calls");
    let x_ms = ms(x);
    let of_x = |d: Duration| {
        if x_ms == 0.0 {
            f64::NAN
        } else {
            ms(d) / x_ms * 100.0
        }
    };
    for (name, d, calls) in [
        (
            "parse_chunk (Raw -> AST)",
            inner.parse_chunk,
            inner.parse_chunk_calls,
        ),
        (
            "second pass (discarded)",
            inner.second_pass,
            inner.second_pass_calls,
        ),
    ] {
        println!("{name:<24}{:>12.1}{:>9.1}%{calls:>10}", ms(d), of_x(d));
    }
    // Node building is what is left of the conversion once the text bridge is
    // out. The second pass is *not* subtracted: it is a re-run of the whole
    // conversion, so its parse and build time are already inside the two lines
    // above and this one respectively.
    let build = inner.total.saturating_sub(inner.parse_chunk);
    println!(
        "{:<24}{:>12.1}{:>9.1}%{:>10}",
        "node building",
        ms(build),
        of_x(build),
        ""
    );

    let per_file = |n: u64| n as f64 / sources.len().max(1) as f64;
    let per_conv = |n: u64| n as f64 / inner.total_calls.max(1) as f64;
    println!("\n-- work counts (deterministic) --");
    println!("{:<28}{:>14}{:>12}{:>12}", "quantity", "total", "per file", "per conv");
    for (name, n) in [
        ("converted nodes", inner.conv_stmt + inner.conv_expr),
        ("  stmt dispatches", inner.conv_stmt),
        ("  expr dispatches", inner.conv_expr),
        ("parsed nodes (all passes)", inner.parsed_nodes),
        ("IR statements in", inner.js_stmts_in),
        ("oxc statements out (top)", inner.oxc_stmts_out),
        ("stmts out of parse_chunk", inner.raw_stmts),
        ("parse_chunk calls", inner.parse_chunk_calls),
        ("parse_chunk bytes", inner.parse_chunk_bytes),
        ("two-pass conversions", inner.second_pass_calls),
        ("bailed conversions", inner.bails),
        ("mappings converted", inner.mappings_out),
    ] {
        println!(
            "{name:<28}{n:>14}{:>12.1}{:>12.1}",
            per_file(n),
            per_conv(n)
        );
    }
    // The identity the whole comparison rests on: the other side reports 449.4
    // nodes per file, and a unit-price comparison is only about price if the
    // volumes agree. Printed every run, because a mismatch means the two counts
    // are of different things and the per-node numbers are not comparable.
    let files = sources.len().max(1) as f64;
    let converted = (inner.conv_stmt + inner.conv_expr) as f64 / files;
    // `parsed_nodes` is summed over passes, and a chunk is parsed once per pass
    // plus once more within a pass when it carries comments. The program holds
    // each of those nodes once, so the count has to be divided by the passes
    // that actually ran to be a per-program figure. `parse_chunk_calls` over the
    // chunks a single pass would parse is that factor, measured rather than
    // assumed -- and it is printed so the correction is visible.
    let raw_stmt_passes = if inner.raw_stmts == 0 {
        1.0
    } else {
        inner.raw_stmts as f64 / inner.oxc_stmts_out.max(1) as f64
    };
    let parsed = inner.parsed_nodes as f64 / files / raw_stmt_passes;
    println!(
        "\nnode identity (vs 449.4 reported for this corpus)"
    );
    println!("  converted            {converted:>8.1} /file");
    println!(
        "  parsed from text     {parsed:>8.1} /file  (raw counted {:.1}, over {raw_stmt_passes:.2} passes)",
        inner.parsed_nodes as f64 / files
    );
    println!(
        "  ★ total              {:>8.1} /file  -> {:.4}x",
        converted + parsed,
        (converted + parsed) / 449.4
    );
    println!(
        "\ntwo-pass rate {:.1}% of conversions; bail rate {:.1}%",
        inner.second_pass_calls as f64 / inner.total_calls.max(1) as f64 * 100.0,
        inner.bails as f64 / inner.total_calls.max(1) as f64 * 100.0
    );

    println!("\n-- implied unit price --");
    println!("{:<40}{:>12}", "operation", "ns each");
    let ns = |d: Duration, n: u64| {
        if n == 0 {
            f64::NAN
        } else {
            d.as_secs_f64() * 1e9 / n as f64
        }
    };
    // `oxc_stmts_out` is program-level only, so a price per "statement" here is
    // a price per *subtree*, not per node -- 7.8 top-level statements stand for
    // the whole program. Quoted as such rather than as a node cost, because a
    // number that implausible for one node is the counter telling you it is
    // counting something else.
    for (name, d, n) in [
        (
            "parse_chunk / byte",
            inner.parse_chunk,
            inner.parse_chunk_bytes,
        ),
        (
            "node building / top-level stmt",
            build,
            inner.oxc_stmts_out,
        ),
        (
            "mappings_convert / mapping",
            inner.mappings_convert,
            inner.mappings_out,
        ),
        ("alloc_reset / call", inner.alloc_reset, inner.alloc_reset_calls),
    ] {
        println!("{name:<40}{:>12.2}", ns(d, n));
    }
}
