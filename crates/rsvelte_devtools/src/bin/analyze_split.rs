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
//! bucket) and **takes** its own six buckets, so it must run before
//! `take_pipeline_breakdown`, which clears that parent. This binary reads them
//! in that order and then prints the agreement between the two as a check: the
//! parent is one counter read twice, so a non-zero difference means a read went
//! out of order, not that the phases disagree.
//!
//! # The residual is the point
//!
//! The buckets name six calls. `analyze_component` also does a large amount of
//! inline work between them, and that lands in `unattributed`. A residual that
//! dominates is a finding, not a defect in the split: it says the phase's cost
//! is not in the calls anyone would think to name.
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
    ] {
        println!("{name:<18}{:>12.1}{:>9.1}%{calls:>10}", ms(d), pct(d));
    }
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
    for (i, name) in [
        "extract_scripts",
        "create_scopes",
        "store_subs",
        "template",
        "css_analyze",
        "css_scope",
        "residual",
    ]
    .iter()
    .enumerate()
    {
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
    println!(
        "blanked bytes {} ({:.0} per blanking call, source is {:.0} per file)",
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
