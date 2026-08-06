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

    // Sampled after the first timed compile, so the arena gate can ask that the
    // peak not move for the remaining N-1. The warm-up above has already brought
    // a working arena to its steady state, so a reset that frees leaves this
    // flat; a reset that is missing keeps growing through the loop.
    //
    // Two weaker forms were tried first and both passed the negative control:
    // a fixed 64 MiB ceiling (cleared by 17 KB while the arena grew 64x), and
    // peak(N/2) == peak(N) -- which fails because the growth *saturates*, so
    // "grows with N" is not true even when the reset is gone.
    //
    // The fragility to name: one file far larger than the warm-up's range could
    // legitimately grow the arena once and trip this. On flowbite it does not --
    // the peak is flat at 1,048,512 B across all 1296, 26 KB max file included.
    let mut arena_peak_first = 0;
    for (index, source) in sources.iter().enumerate() {
        if index == 1 {
            arena_peak_first = profile::peek_codegen_arena_peak();
        }
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
            // Not "(discarded)": what gets thrown away is the *first* pass of
            // these programs. This line is the re-run that replaces it, and it
            // is the more expensive of the two, since only this pass re-parses
            // comment-bearing chunks.
            "second pass (the re-run)",
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
    // -- pass-matched view --------------------------------------------------
    //
    // `parse_chunk` and `second_pass` are not disjoint: the second pass parses
    // every chunk again, so its parse time sits inside both lines. Everything
    // below is stated over the *first* pass alone, which every program runs
    // exactly once -- that removes the overlap by construction instead of
    // estimating it, and gives the two per-node prices the same denominator.
    let files = sources.len().max(1) as f64;
    let conv_all = inner.conv_stmt + inner.conv_expr;
    let conv_sp = inner.sp_conv_stmt + inner.sp_conv_expr;
    let first_parse = inner.parse_chunk.saturating_sub(inner.sp_parse_chunk);
    let first_total = inner.total.saturating_sub(inner.second_pass);
    let first_build = first_total.saturating_sub(first_parse);
    let first_conv = conv_all.saturating_sub(conv_sp);
    let first_parsed = inner.parsed_nodes.saturating_sub(inner.sp_parsed_nodes);

    println!("\n-- per pass (second pass is a SUBSET of the totals above) --");
    println!(
        "{:<28}{:>14}{:>14}{:>14}",
        "quantity", "all passes", "second pass", "first pass"
    );
    let row = |name: &str, all: u64, sp: u64| {
        println!(
            "{name:<28}{all:>14}{sp:>14}{:>14}",
            all.saturating_sub(sp)
        );
    };
    row("converted nodes", conv_all, conv_sp);
    row("parsed nodes", inner.parsed_nodes, inner.sp_parsed_nodes);
    row(
        "parse_chunk calls",
        inner.parse_chunk_calls,
        inner.sp_parse_chunk_calls,
    );
    row(
        "parse_chunk bytes",
        inner.parse_chunk_bytes,
        inner.sp_parse_chunk_bytes,
    );
    println!(
        "{:<28}{:>14.1}{:>14.1}{:>14.1}",
        "parse_chunk ms",
        ms(inner.parse_chunk),
        ms(inner.sp_parse_chunk),
        ms(first_parse)
    );
    // The subset invariant, printed rather than assumed. A violation would mean
    // the flag is being read in a pass it does not describe, which is exactly
    // the failure this split exists to rule out.
    let subset_ok = inner.sp_parse_chunk <= inner.parse_chunk
        && inner.sp_parse_chunk <= inner.second_pass
        && conv_sp <= conv_all
        && inner.sp_parsed_nodes <= inner.parsed_nodes;
    println!(
        "subset invariant (second <= total, and second-pass parse <= second pass): {}",
        if subset_ok { "ok" } else { "VIOLATED" }
    );
    // (6): how much of the second pass is parsing. Quoted so `parse_chunk` and
    // `second_pass` are never added together.
    println!(
        "overlap: parse_chunk inside second pass = {:.1} ms = {:.1}% of X, {:.1}% of the second pass",
        ms(inner.sp_parse_chunk),
        of_x(inner.sp_parse_chunk),
        inner.sp_parse_chunk.as_secs_f64() / inner.second_pass.as_secs_f64().max(f64::MIN_POSITIVE)
            * 100.0
    );

    // The two prices the text-carried-vs-structured reading rests on, both over
    // the first pass so neither is inflated by a re-run the other did not have.
    println!("\n-- per-node price, first pass only --");
    let per_node = |d: Duration, n: u64| {
        if n == 0 {
            f64::NAN
        } else {
            d.as_secs_f64() * 1e9 / n as f64
        }
    };
    let text_price = per_node(first_parse, first_parsed);
    let struct_price = per_node(first_build, first_conv);
    println!(
        "  text-carried (parse_chunk)   {text_price:>8.1} ns/node   over {:.1} nodes/file",
        first_parsed as f64 / files
    );
    println!(
        "  structured (node building)   {struct_price:>8.1} ns/node   over {:.1} nodes/file",
        first_conv as f64 / files
    );
    println!("  ratio                        {:>8.2}x", text_price / struct_price);

    // -- where the text-carried nodes came from -----------------------------
    //
    // `RawMapped` is emitted at three sites and all three are the instance
    // script, which phase 2 has already parsed into a retained oxc program.
    // Those nodes would not have to be built again if the text carrier went
    // away; module-script and template nodes would. `Raw` cannot be used for
    // this -- the template visitors emit it too.
    //
    // Read over the first pass, for the same reason every other node count is.
    let first_mapped = inner
        .mapped_parsed_nodes
        .saturating_sub(inner.sp_mapped_parsed_nodes);
    let first_other = first_parsed.saturating_sub(first_mapped);
    let n = first_mapped as f64 / files;
    let text_nodes = first_parsed as f64 / files;
    // Rebuild cost for exactly the nodes that have no retained AST behind them.
    // Conditional on the arena/lifetime obstacle being solved: while a retained
    // program owns its own allocator, reusing its nodes means a deep copy, and
    // the copy is a per-node cost of its own rather than zero.
    let c = struct_price * (text_nodes - n) / 1000.0;
    println!("\n-- origin of the text-carried nodes (first pass) --");
    println!(
        "  instance script (RawMapped)  {n:>8.1} /file   = {:.1}% of the {text_nodes:.1} text-carried",
        if text_nodes > 0.0 { n / text_nodes * 100.0 } else { f64::NAN }
    );
    println!(
        "  module script + template     {:>8.1} /file",
        first_other as f64 / files
    );
    println!(
        "  (c) = {struct_price:.1} ns x ({text_nodes:.1} - {n:.1}) = {c:.2} us/file   [assumes the arena obstacle is solved]"
    );
    println!(
        "  closure: mapped {first_mapped} + other {first_other} = {} vs parsed(first pass) {first_parsed} -> {}",
        first_mapped + first_other,
        if first_mapped + first_other == first_parsed { "ok" } else { "VIOLATED" }
    );

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
    // Kept for continuity with the earlier runs, but NOT an identity to close.
    // 449.4 counts ESTree nodes in the *printed output*, re-parsed with acorn;
    // these count dispatches taken while *building*. The two differ in both
    // directions -- a discarded pass adds to the build side without adding to
    // the output, and text-carried nodes reach the output without a dispatch --
    // so there is no arithmetic that makes them meet, and forcing one would only
    // amount to choosing the projection that fits.
    println!(
        "\nbuild-side node count (NOT commensurable with the reported 449.4)"
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
    // The resource gate. The reset moved from the code that allocates into the
    // entries, and a path that reaches codegen without an entry would leave the
    // arena growing for the life of the process -- while the output stays
    // byte-identical and every work count stays put. Neither sha256 nor the
    // deterministic counters can see that; these two numbers can.
    //
    // The denominator is files, not `total_calls`: the latter counts
    // `program_to_oxc` invocations, and a component with no script never gets
    // that far while still being compiled -- and still resetting. Comparing
    // against it reported a missing reset on the first run of this gate, when
    // what was actually wrong was the denominator.
    let compiles = sources.len() as u64;
    println!(
        "\narena: resets {} vs compiles {} -> {} ; peak capacity {} B after 1 compile -> {} B after {} over {} samples -> {}",
        inner.arena_resets,
        compiles,
        if inner.arena_resets == compiles { "ok" } else { "RESET COUNT OFF" },
        arena_peak_first,
        inner.arena_capacity,
        compiles,
        inner.arena_samples,
        // A per-compile arena keeps a warm chunk; one that is never freed grows
        // with the corpus. Comparing the peak at N/2 with the peak at N asks
        // that directly, with no constant to choose -- the first version of this
        // gate used a "deliberately loose" 64 MiB ceiling and the negative
        // control cleared it by 17 KB while growing the arena 64x.
        //
        // The sample count is checked first. Without it, a run in which nothing
        // ever sampled the arena reports a peak of 0 and passes -- which is what
        // the first negative control for this gate actually produced.
        if inner.arena_samples != compiles {
            "NO EVIDENCE"
        } else if inner.arena_capacity == arena_peak_first {
            "ok"
        } else {
            "GROWING"
        }
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
