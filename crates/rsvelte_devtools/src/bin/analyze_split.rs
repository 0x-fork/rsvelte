//! Sub-split of the analyze phase, taken from inside the production pipeline.
//!
//! Same construction as `pipeline_split`: the timers sit on the production call
//! sites inside `analyze_component`, so there is no orchestration to get wrong.
//! This binary exists to make the instrument's output readable without building
//! the napi addon — the napi consumer is `takeAnalyzeSplit`, and a comparison
//! against another compiler should go through that so one harness drives both
//! in the same minute.
//!
//! # Read the ordering constraint before trusting a number
//!
//! `take_analyze_breakdown` **peeks** its parent (the pipeline's `analyze`
//! bucket) and **takes** its own eleven buckets, so it must run before
//! `take_pipeline_breakdown`, which clears that parent. This binary reads them
//! in that order and then prints the agreement between the two as a check: the
//! parent is one counter read twice, so a non-zero difference means a read went
//! out of order, not that the phases disagree.
//!
//! # The residual is the point
//!
//! The first six buckets name six calls. `analyze_component` also does a large
//! amount of inline work between them, and in the first split that landed in
//! `unattributed` and turned out to be the largest bucket on both time and walk
//! volume -- a finding, not a defect: it said the phase's cost is not in the
//! calls anyone would think to name.
//!
//! `R1`-`R5` now cover that inline code in source order, so `unattributed` here
//! is a residual-of-residual. It is still printed, and still printed as a share
//! of the parent, because the way that finding would be lost is by folding the
//! unnamed part into whichever neighbour it sits next to.
//!
//! ```text
//! cargo run --release -p rsvelte_devtools --bin analyze_split -- <dir>...
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

/// Index order of the per-bucket visit and parse arrays, matching
/// `profile::AnalyzeBucket`. Declared once because the two tables below index
/// the same arrays, and two copies of an order is one copy that can drift.
const BUCKET_NAMES: [&str; profile::ANALYZE_BUCKETS] = [
    "extract_scripts",
    "create_scopes",
    "store_subs",
    "template",
    "css_analyze",
    "css_scope",
    "R1 setup",
    "R2 feature_detect",
    "R3 visit_scripts",
    "R4 binding_fixups",
    "R5 finalize",
    "residual",
];

fn main() {
    // The timers are off in the shipped compiler, so a profiler has to ask.
    profile::set_timers_enabled(true);
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        eprintln!("usage: analyze_split <dir>...");
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

    // Warm up on the measured entry point, then discard: both readers have to
    // be drained, or the warm-up's buckets join the timed pass.
    for source in sources.iter().take(100) {
        let _ = compile_with_external_sourcemap_content(source, CompileOptions::default());
    }
    // All three readers, not two. Draining only the timers left the warm-up's
    // visits in the counters, and the timed pass then reported 39,757 template
    // dispatches against 36,866 from the same walk -- the difference was
    // exactly the hundred warm-up files. The napi reader takes both in one
    // call, so this asymmetry is local to this binary.
    let _ = profile::take_store_subs_breakdown();
    let _ = profile::take_feature_detect_breakdown();
    let _ = profile::take_finalize_breakdown();
    let _ = profile::take_analyze_breakdown();
    let _ = profile::take_analyze_visits();
    let _ = profile::take_pipeline_breakdown();

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

    // Order matters: the analyze split peeks the parent that the pipeline split
    // clears. Reversing these two lines reports a zero total, which the check
    // below turns into a visible failure rather than a plausible share table.
    let s = profile::take_store_subs_breakdown();
    let fd = profile::take_feature_detect_breakdown();
    let fin = profile::take_finalize_breakdown();
    let a = profile::take_analyze_breakdown();
    let v = profile::take_analyze_visits();
    let p = profile::take_pipeline_breakdown();

    let total = ms(a.total);
    let pct = |d: Duration| {
        if total == 0.0 {
            f64::NAN
        } else {
            ms(d) / total * 100.0
        }
    };

    println!("{} files, {} compiles", sources.len(), a.compiles);
    println!("{:<18}{:>12}{:>10}{:>10}", "bucket", "ms", "share", "calls");
    for (name, d, calls) in [
        (
            "extract_scripts",
            a.extract_scripts,
            a.extract_scripts_calls,
        ),
        ("create_scopes", a.create_scopes, a.create_scopes_calls),
        ("store_subs", a.store_subs, a.store_subs_calls),
        ("template", a.template, a.template_calls),
        ("css_analyze", a.css_analyze, a.css_analyze_calls),
        ("css_scope", a.css_scope, a.css_scope_calls),
        ("R1 setup", a.setup, a.setup_calls),
        ("R2 feature_detect", a.feature_detect, a.feature_detect_calls),
        ("R3 visit_scripts", a.visit_scripts, a.visit_scripts_calls),
        ("R4 binding_fixups", a.binding_fixups, a.binding_fixups_calls),
        ("R5 finalize", a.finalize, a.finalize_calls),
    ] {
        println!("{name:<18}{:>12.1}{:>9.1}%{calls:>10}", ms(d), pct(d));
    }
    // Residual of residual now: R1-R5 cover the phase's inline code, so what is
    // left is the glue between them. Still printed, and still printed as a
    // share: the first split's finding was that the unnamed part was the
    // largest part, which is only visible while it keeps being reported.
    println!(
        "{:<18}{:>12.1}{:>9.1}%{:>10}",
        "unattributed",
        ms(a.unattributed()),
        pct(a.unattributed()),
        ""
    );
    println!(
        "{:<18}{total:>12.1}{:>9.1}%{:>10}",
        "analyze total", 100.0, ""
    );

    // Template nodes only. Both walks also descend into the scripts and no
    // counter here sees that, so this is walk volume, not a denominator: do not
    // divide a bucket by it and call the result a cost per node.
    println!(
        "\ntemplate nodes dispatched (template only, scripts uncounted): create_scopes {} / analyze_template {}",
        a.create_scopes_nodes, a.template_nodes
    );

    let files = sources.len() as f64;
    let bytes = v.source_bytes as f64;
    println!(
        "\nsource bytes {} ({:.0} per file){}",
        v.source_bytes,
        bytes / files,
        if v.js_counted {
            ""
        } else {
            "   [js slots NOT counted: rebuild with --features measure-analyze-nodes]"
        }
    );
    println!(
        "{:<18}{:>14}{:>14}{:>13}{:>12}",
        "bucket", "tplVisits", "jsSlots", "visits/file", "visits/KB"
    );
    for (i, name) in BUCKET_NAMES.iter().enumerate() {
        let visits = (v.template[i] + v.js[i]) as f64;
        println!(
            "{name:<18}{:>14}{:>14}{:>13.1}{:>12.2}",
            v.template[i],
            v.js[i],
            visits / files,
            if bytes == 0.0 {
                f64::NAN
            } else {
                visits / (bytes / 1024.0)
            }
        );
    }
    // `jsSlots` counts child-slot expansions, not distinct nodes, and a walk
    // that reads one node's children twice charges them twice. Another
    // compiler's "visit" is a different definition, so a cross-compiler ratio
    // has to use the byte denominator, which both sides define identically.
    println!(
        "visits are rsvelte-internal: tplVisits = dispatches, jsSlots = child-slot expansions.\n\
         For a cross-compiler ratio use time per source byte."
    );

    // Inside store_subs, which spends a fifth of the phase without walking the
    // AST. The gate ratio is printed with its denominator: a zero here means
    // the `$`-absent fast path never fired, not that it was not measured.
    let ss_total = ms(a.store_subs);
    let ss_pct = |d: Duration| {
        if ss_total == 0.0 {
            f64::NAN
        } else {
            ms(d) / ss_total * 100.0
        }
    };
    // The `FeatureDetect` walks, and the subset of them whose answer was fixed
    // before they started. `wasted` is the number a lever would act on; the two
    // above it are there so the share has a denominator and the step from
    // "ran" to "could not have found anything" is visible rather than asserted.
    let fd_pct = |n: u64, d: u64| {
        if d == 0 {
            f64::NAN
        } else {
            n as f64 / d as f64 * 100.0
        }
    };
    println!(
        "\nfeature_detect: {} compiles, gate passed {} ({:.2}%), needs_rune {} ({:.2}%), await in source {} ({:.2}%)",
        fd.calls,
        fd.gate_passed,
        fd_pct(fd.gate_passed, fd.calls),
        fd.needs_rune,
        fd_pct(fd.needs_rune, fd.calls),
        fd.await_in_source,
        fd_pct(fd.await_in_source, fd.calls),
    );
    println!(
        "{:<20}{:>10}{:>14}{:>12}{:>10}",
        "walk", "ran", "await-only", "wasted", "wasted%"
    );
    for (name, ran, await_only, wasted) in [
        (
            "instance script",
            fd.instance_walks,
            fd.instance_walks_await_only,
            fd.instance_walks_wasted,
        ),
        (
            "template fragment",
            fd.fragment_walks,
            fd.fragment_walks_await_only,
            fd.fragment_walks_wasted,
        ),
    ] {
        println!(
            "{name:<20}{ran:>10}{await_only:>14}{wasted:>12}{:>9.2}%",
            fd_pct(wasted, ran)
        );
    }

    // No gate to report here: `ScopeRoot::conflicts` is consumed by Phase 3 for
    // every component, so this walk's output is always live. What the pair says
    // is how much of the product survives the filter -- a walk that collects
    // many to keep few is a candidate for being folded into the scope builder's
    // walk over the same scripts, not for being skipped.
    println!(
        "\nfinalize: {} compiles, names collected {} ({:.1}/file), surviving {} ({:.1}/file, {:.2}% kept), component renamed {} ({:.2}%)",
        fin.calls,
        fin.names_collected,
        fin.names_collected as f64 / files,
        fin.names_surviving,
        fin.names_surviving as f64 / files,
        if fin.names_collected == 0 {
            f64::NAN
        } else {
            fin.names_surviving as f64 / fin.names_collected as f64 * 100.0
        },
        fin.name_deconflicted,
        if fin.calls == 0 {
            f64::NAN
        } else {
            fin.name_deconflicted as f64 / fin.calls as f64 * 100.0
        },
    );

    println!(
        "\nstore_subs: {} calls, gate skipped {} ({:.2}% of calls)",
        s.calls,
        s.gate_skipped,
        if s.calls == 0 {
            f64::NAN
        } else {
            s.gate_skipped as f64 / s.calls as f64 * 100.0
        }
    );
    println!("{:<22}{:>12}{:>10}{:>10}", "part", "ms", "share", "calls");
    for (name, d, calls) in [
        ("blank_typescript", s.blank_ts, s.blank_ts_calls),
        ("lexical scan", s.lex_scan, s.lex_scan_calls),
        ("fragment recursion", s.fragment, s.fragment_calls),
    ] {
        println!("{name:<22}{:>12.1}{:>9.1}%{calls:>10}", ms(d), ss_pct(d));
    }
    let ss_rest = a
        .store_subs
        .saturating_sub(s.blank_ts + s.lex_scan + s.fragment);
    println!(
        "{:<22}{:>12.1}{:>9.1}%{:>10}",
        "rest of the fn",
        ms(ss_rest),
        ss_pct(ss_rest),
        ""
    );
    println!(
        "{:<22}{ss_total:>12.1}{:>9.1}%{:>10}",
        "store_subs total", 100.0, ""
    );
    // `blank_typescript` is an oxc TypeScript parse plus a full AST visit; the
    // byte blanking is only its last branch, and both early exits have already
    // paid for the parse. The exit mix is what says how often that parse bought
    // nothing but a `source.to_string()`.
    println!(
        "blank_typescript exits: parse-failed {} / nothing-to-blank {} / blanked {}",
        s.blank_diag_exits, s.blank_empty_exits, s.blank_blanked_exits
    );
    // Analyze parses the same scripts again and no reparse counter sees it: all
    // fifteen `record_reparse` / `record_direct_parse` sites are under
    // `3_transform/`, none under `2_analyze/`.
    let parses: u64 = v.parse_calls.iter().sum();
    let parsed_bytes: u64 = v.parse_bytes.iter().sum();
    println!(
        "analyze-side oxc parses {} ({:.2}/file, {} bytes = {:.0}% of source re-read)",
        parses,
        parses as f64 / files,
        parsed_bytes,
        if bytes == 0.0 {
            f64::NAN
        } else {
            parsed_bytes as f64 / bytes * 100.0
        }
    );
    for (i, name) in BUCKET_NAMES.iter().enumerate() {
        if v.parse_calls[i] != 0 {
            println!(
                "  {name:<18}parses {:>6}   parsedBytes {:>10}",
                v.parse_calls[i], v.parse_bytes[i]
            );
        }
    }
    println!(
        "bytes handed to blank_typescript {} ({:.0} per call, source is {:.0} per file)",
        s.blanked_bytes,
        if s.blank_ts_calls == 0 {
            f64::NAN
        } else {
            s.blanked_bytes as f64 / s.blank_ts_calls as f64
        },
        bytes / files
    );

    // One counter read twice. Any difference is a read-order bug in this
    // binary, not a property of the compiler, so it is worth failing loudly.
    let parent_delta = ms(p.analyze) - total;
    println!(
        "\nparent check: pipeline.analyze {:.1} ms - analyze.total {total:.1} ms = {parent_delta:.6} ms",
        ms(p.analyze),
    );
    if p.analyze != a.total {
        eprintln!("PARENT MISMATCH: the two reads disagree; check the call order");
        std::process::exit(1);
    }
}
