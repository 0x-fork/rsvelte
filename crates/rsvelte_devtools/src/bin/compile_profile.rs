//! Development profiler for the parse, analyze, and transform phases.

// Defined per-bin rather than once in the lib so that linking the `rsvelte_core`
// rlib never imposes an allocator on the consumer.
#[cfg(all(
    feature = "mimalloc-alloc",
    not(target_arch = "wasm32"),
    not(target_os = "windows")
))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(
    feature = "jemalloc",
    not(feature = "mimalloc-alloc"),
    not(target_arch = "wasm32"),
    not(target_os = "windows")
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use rsvelte_core::compiler::phases::phase1_parse::{ParseOptions, parse};
use rsvelte_core::compiler::phases::phase3_transform::profile;
use rsvelte_core::{CompileOptions, GenerateMode};

fn main() {
    // The timers are off in the shipped compiler, so a profiler has to ask.
    profile::set_timers_enabled(true);
    let files = collect_files();
    let total_bytes: usize = files.iter().map(|(_, c)| c.len()).sum();
    println!("Files: {}, Total: {} bytes\n", files.len(), total_bytes);

    let parse_opts = ParseOptions {
        modern: true,
        skip_expression_loc: true,
        defer_script_parse: true,
        ..Default::default()
    };

    // `CompileOptions` defaults this on, so every figure this profiler has ever
    // printed includes source-map generation. The flag exists to measure what
    // that costs, and where: a cost paid while printing lands in `codegen`, not
    // in the wrapper's own bucket.
    let sourcemap = !std::env::args().any(|a| a == "--no-sourcemap");
    let compile_opts = CompileOptions {
        generate: GenerateMode::Client,
        enable_sourcemap: sourcemap,
        ..Default::default()
    };
    println!("sourcemap: {sourcemap}");

    // Warmup
    for (_, content) in files.iter().take(100) {
        let _ = rsvelte_core::compile(content, compile_opts.clone());
    }

    // One production compile per file, with the phase split read back from the
    // pipeline's own timers.
    //
    // This profiler used to stage the phases by hand -- parse, then
    // resolve_lazy, then ensure_script, then analyze, then transform -- to time
    // each one. That diverged from production four ways at once: no retained
    // scripts, no TypeScript removal, no `<svelte:options>` merge, and
    // `compute_line_offsets(_, false)` where production passes the AST's flag.
    // Two of those inflate a bucket and one shrinks the denominator, so the
    // shares were not bounded in a single direction. Driving `compile` removes
    // the orchestration and with it the possibility of getting it wrong.
    let mut totals = profile::Phase3Breakdown::default();
    let mut pipeline = profile::PipelineBreakdown::default();
    // Per-file rows, so re-parse cost can be read against file size instead of
    // only as one corpus-wide average.
    let mut rows: Vec<(usize, std::time::Duration, profile::ReparseBreakdown)> =
        Vec::with_capacity(files.len());
    let mut scaling: Vec<ScalingRow> = Vec::with_capacity(files.len());

    // Drain whatever the warmup left, so the first file does not inherit it.
    let _ = profile::take_breakdown();
    let _ = profile::take_reparse_breakdown();
    let _ = profile::take_pipeline_breakdown();
    let _ = profile::take_script_text_breakdown();
    let _ = profile::take_ast_transforms_breakdown();
    let _ = profile::take_template_fragment_breakdown();
    let _ = profile::take_assembly_breakdown();
    let _ = profile::take_residual_breakdown();
    let _ = profile::take_scan_counts();
    let _ = profile::take_rewrite_counts();
    let _ = profile::take_collect_vars_breakdown();
    let _ = profile::take_text_identity();

    // A failing compile leaves several phase timers unrecorded: the `?` on the
    // phase call returns before the recorder runs, so that compile's parse,
    // ensure-script, TS-removal, analyze, transform and CSS time is lost while
    // its total is not. The count is the denominator that says whether that
    // matters here.
    let mut failed = 0usize;
    let mut scan_total = profile::ScanCounts::default();
    for (_, content) in &files {
        if rsvelte_core::compile(content, compile_opts.clone()).is_err() {
            failed += 1;
        }

        // Drained per file for the scaling rows, so the corpus totals have to be
        // accumulated here rather than read back after the loop.
        let b = profile::take_breakdown();
        let p = profile::take_pipeline_breakdown();
        totals.visit_program += b.visit_program;
        totals.script_text_transform += b.script_text_transform;
        totals.template_fragment += b.template_fragment;
        totals.assembly_after_fragment += b.assembly_after_fragment;
        totals.css_render += b.css_render;
        totals.codegen += b.codegen;
        pipeline.parse += p.parse;
        pipeline.line_offsets += p.line_offsets;
        pipeline.resolve_lazy += p.resolve_lazy;
        pipeline.ensure_script += p.ensure_script;
        pipeline.ts_removal += p.ts_removal;
        pipeline.options_merge += p.options_merge;
        pipeline.analyze += p.analyze;
        pipeline.transform += p.transform;
        pipeline.finalize += p.finalize;
        pipeline.total += p.total;
        pipeline.compiles += p.compiles;

        // Labels only. Parsing again here costs a second parse per file, but it
        // is outside every timer, and the alternative -- keeping the staged AST
        // alive to read it -- is the staging this rewrite removed.
        let arena = oxc_allocator::Allocator::default();
        let ast = parse(content, &arena, parse_opts).ok();
        let (script_bytes, runes) = script_shape(ast.as_ref(), content);
        let r = profile::take_reparse_breakdown();
        let sc = profile::take_scan_counts();
        scan_total.bytes += sc.bytes;
        scan_total.calls += sc.calls;
        scan_total.script_bytes += sc.script_bytes;
        for i in 0..profile::SCAN_SITE_COUNT {
            scan_total.site_bytes[i] += sc.site_bytes[i];
            scan_total.site_calls[i] += sc.site_calls[i];
        }
        scaling.push(ScalingRow {
            file_bytes: content.len(),
            script_bytes,
            ensure_script: p.ensure_script,
            runes,
            analyze: p.analyze,
            script_text: b.script_text_transform,
            template: b.template_fragment,
            codegen: b.codegen,
            transform: p.transform,
            // Re-parse volume against the script text is the one figure here
            // that no clock enters, so it can be read on a loaded machine.
            reparse_bytes: r.bytes + r.direct_bytes,
            reparse_calls: r.calls + r.direct_calls,
            reparse_parse: r.parse,
            direct_parse: r.direct_parse,
            visit_program: b.visit_program,
            assembly: b.assembly_after_fragment,
            css_render: b.css_render,
            scan_bytes: sc.bytes,
            scan_calls: sc.calls,
            scan_staged_bytes: sc.site_bytes[profile::SCAN_SITE_STAGED],
        });
        rows.push((content.len(), p.transform, r));
    }

    let parse_time = pipeline.parse;
    let resolve_lazy_time = pipeline.resolve_lazy;
    let ensure_script_time = pipeline.ensure_script;
    let analyze_visitor_time = pipeline.analyze;
    let analyze_time = resolve_lazy_time + ensure_script_time + analyze_visitor_time;
    let transform_time = pipeline.transform;
    let transform_breakdown = totals;
    let script_text_breakdown = profile::take_script_text_breakdown();
    let at = profile::take_ast_transforms_breakdown();
    let tf = profile::take_template_fragment_breakdown();
    let asm = profile::take_assembly_breakdown();
    let rs = profile::take_residual_breakdown();
    let scan = scan_total;
    let rw = profile::take_rewrite_counts();
    let cv = profile::take_collect_vars_breakdown();
    let ti = profile::take_text_identity();

    // The whole compile, measured independently of the buckets, so a phase
    // nobody instrumented lands in the residual instead of inflating a share.
    let total = pipeline.total;
    let pct = |d: std::time::Duration| d.as_secs_f64() / total.as_secs_f64() * 100.0;
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;

    println!("=== Compile Phase Breakdown ===");
    println!(
        "Phase 1 (Parse):       {:7.2}ms ({:5.1}%)",
        ms(parse_time),
        pct(parse_time)
    );
    println!(
        "Phase 2 (Analyze):     {:7.2}ms ({:5.1}%)",
        ms(analyze_time),
        pct(analyze_time)
    );
    println!(
        "  Resolve lazy:        {:7.2}ms ({:5.1}%)",
        ms(resolve_lazy_time),
        pct(resolve_lazy_time)
    );
    println!(
        "  Ensure script (OXC): {:7.2}ms ({:5.1}%)",
        ms(ensure_script_time),
        pct(ensure_script_time)
    );
    println!(
        "  Visitors (rest):     {:7.2}ms ({:5.1}%)",
        ms(analyze_visitor_time),
        pct(analyze_visitor_time)
    );
    // Rows the hand-staged version had no way to print, because it never ran
    // the steps they measure.
    for (label, d) in [
        ("Line offsets:", pipeline.line_offsets),
        ("TS removal:", pipeline.ts_removal),
        ("Options merge:", pipeline.options_merge),
        ("Finalize result:", pipeline.finalize),
    ] {
        println!("{label:<22} {:7.2}ms ({:5.1}%)", ms(d), pct(d));
    }
    println!(
        "Phase 3 (Transform):   {:7.2}ms ({:5.1}%)",
        ms(transform_time),
        pct(transform_time)
    );
    let visit_program = transform_breakdown.visit_program;
    let script_text = transform_breakdown.script_text_transform;
    let template_fragment = transform_breakdown.template_fragment;
    let assembly_after = transform_breakdown.assembly_after_fragment;
    let css_render = transform_breakdown.css_render;
    let codegen = transform_breakdown.codegen;
    let other = transform_time
        .saturating_sub(visit_program)
        .saturating_sub(script_text)
        .saturating_sub(template_fragment)
        .saturating_sub(assembly_after)
        .saturating_sub(css_render)
        .saturating_sub(codegen);
    println!(
        "  visit_program:       {:7.2}ms ({:5.1}%)",
        ms(visit_program),
        pct(visit_program)
    );
    println!(
        "  Script-text xform:   {:7.2}ms ({:5.1}%)",
        ms(script_text),
        pct(script_text)
    );
    let st = script_text_breakdown;
    // Residual rows are signed: saturating them to zero would hide the very
    // inconsistency the self-check below exists to expose.
    let residual = |whole: std::time::Duration, parts: &[std::time::Duration]| {
        ms(whole) - parts.iter().copied().map(ms).sum::<f64>()
    };
    for (label, val) in [
        ("prenormalize", ms(st.prenormalize)),
        ("collect_vars", ms(st.collect_vars)),
        ("line_loop", ms(st.line_loop)),
        ("  process_accum", ms(st.process_accumulated)),
        ("    pa_prologue", ms(st.pa_prologue)),
        ("    runes_xform", ms(st.runes)),
        ("    reactive_stmt", ms(st.reactive_stmt)),
        (
            "    pa_rest",
            residual(
                st.process_accumulated,
                &[st.pa_prologue, st.runes, st.reactive_stmt],
            ),
        ),
        (
            "  line_scan",
            residual(st.line_loop, &[st.process_accumulated]),
        ),
        ("ast_transforms", ms(st.ast_transforms)),
        ("  at_probe", ms(at.probe)),
        ("  at_parse", ms(at.parse)),
        ("  at_walk", ms(at.walk)),
        ("  at_output", ms(at.output)),
        ("  at_store_unsub", ms(at.store_unsub)),
        (
            "  at_rest",
            residual(
                st.ast_transforms,
                &[at.probe, at.parse, at.walk, at.output, at.store_unsub],
            ),
        ),
        ("post_passes", ms(st.post_passes)),
        (
            "prologue+earlyout",
            residual(
                script_text,
                &[
                    st.prenormalize,
                    st.collect_vars,
                    st.line_loop,
                    st.ast_transforms,
                    st.post_passes,
                ],
            ),
        ),
    ] {
        println!(
            "    {label:<18} {val:7.2}ms ({:5.1}%)",
            val / ms(total) * 100.0
        );
    }
    println!("    (statements processed: {})", st.statements);
    // Load-independent: the question a 15x ratio asks is how many times the
    // pipeline walks its input, not how long each walk took.
    println!(
        "    SCANS  {} calls over {} bytes vs script {} bytes = {:.2} effective passes ({:.1} calls/file)",
        scan.calls,
        scan.bytes,
        scan.script_bytes,
        if scan.script_bytes > 0 {
            scan.bytes as f64 / scan.script_bytes as f64
        } else {
            0.0
        },
        scan.calls as f64 / files.len() as f64
    );
    for (label, val) in [
        ("cv_analysis_vecs", ms(cv.analysis_vecs)),
        ("cv_text_index", ms(cv.text_index)),
        ("cv_proxy_vars", ms(cv.proxy_vars)),
        ("cv_binding_vecs", ms(cv.binding_vecs)),
        ("cv_set_maps", ms(cv.set_maps)),
        ("cv_line_split", ms(cv.line_split)),
    ] {
        println!(
            "      {label:<18} {val:7.2}ms ({:5.1}% of collect_vars)",
            val / ms(st.collect_vars).max(f64::MIN_POSITIVE) * 100.0
        );
    }
    println!(
        "      cv IDENTITY sum {:.2}ms vs parent {:.2}ms ({:+.3}ms) | calls {:?} vs staged {}",
        ms(cv.analysis_vecs)
            + ms(cv.text_index)
            + ms(cv.proxy_vars)
            + ms(cv.binding_vecs)
            + ms(cv.set_maps)
            + ms(cv.line_split),
        ms(st.collect_vars),
        ms(cv.analysis_vecs)
            + ms(cv.text_index)
            + ms(cv.proxy_vars)
            + ms(cv.binding_vecs)
            + ms(cv.set_maps)
            + ms(cv.line_split)
            - ms(st.collect_vars),
        cv.calls,
        st.calls
    );
    let pn_any =
        rw.files[rsvelte_core::compiler::phases::phase3_transform::profile::REWRITE_PN_ANY];
    println!(
        "      TEXT-IDENTITY checked {} (vs staged {}) | changed {} ({:.1}%) | pn_ANY {} | unexplained {} | noop {} | {}",
        ti.checked,
        st.calls,
        ti.changed,
        ti.changed as f64 / ti.checked.max(1) as f64 * 100.0,
        pn_any,
        ti.unexplained,
        ti.noop,
        if ti.unexplained == 0 && ti.changed + ti.noop == pn_any {
            "PASS: every change is a named site"
        } else if ti.unexplained > 0 {
            "FAIL: an unnamed rewrite path"
        } else {
            "FAIL: the site counts do not close"
        }
    );
    for (i, name) in rsvelte_core::compiler::phases::phase3_transform::profile::REWRITE_SITE_NAMES
        .iter()
        .enumerate()
    {
        println!(
            "      REWRITE {name:<18} {:8} calls {:8} files ({:5.1}% of staged)",
            rw.calls[i],
            rw.files[i],
            rw.files[i] as f64 / st.calls.max(1) as f64 * 100.0
        );
    }
    for (i, name) in rsvelte_core::compiler::phases::phase3_transform::profile::SCAN_SITE_NAMES
        .iter()
        .enumerate()
    {
        println!(
            "      SCANSITE {name:<18} {:9} calls {:12} bytes = {:6.2} passes",
            scan.site_calls[i],
            scan.site_bytes[i],
            if scan.script_bytes > 0 {
                scan.site_bytes[i] as f64 / scan.script_bytes as f64
            } else {
                0.0
            }
        );
    }
    // The verdict the split exists for: only parse and output can go away if
    // this stage is fed an AST instead of the text pipeline's output. There is
    // no print column -- the stage splices into a string, it never serialises.
    let removable = ms(at.parse) + ms(at.output);
    println!(
        "    REMOVABLE-IF-AST-INPUT parse+output {:.3}ms = {:.2}% of ast_transforms | parse_calls {} walk_calls {}",
        removable,
        removable / ms(st.ast_transforms) * 100.0,
        at.parse_calls,
        at.walk_calls
    );
    let st_sum =
        st.prenormalize + st.collect_vars + st.line_loop + st.ast_transforms + st.post_passes;
    println!(
        "    SELF-CHECK sum {:.2}ms vs parent {:.2}ms ({:+.2}ms) | entries {} parent_calls {} staged {}",
        ms(st_sum),
        ms(script_text),
        ms(st_sum) - ms(script_text),
        st.entries,
        st.parent_calls,
        st.calls
    );
    println!(
        "    NESTING nested {} | sites: main {} pub {} (sum {} vs entries {}) | in_function {:.2}ms",
        st.nested_entries,
        st.parent_site_main,
        st.parent_site_pub,
        st.parent_site_main + st.parent_site_pub,
        st.entries,
        ms(st.in_function)
    );
    println!(
        "    PAIRING entries_outside_parent {}",
        st.entries_outside_parent
    );
    report_reparse(&mut rows, ms(total));
    if let Some(path) = std::env::args()
        .position(|a| a == "--dump-rows")
        .and_then(|i| std::env::args().nth(i + 1))
    {
        dump_rows(&scaling, &path);
    }
    report_scan_bands(&scaling);
    report_scaling(&scaling, "script bytes", |r| r.script_bytes as f64);
    report_scaling(&scaling, "rune count", |r| r.runes as f64);
    let oracle = profile::take_index_oracle();
    println!(
        "  index oracle: {} checks, {} mismatches{}",
        oracle.checks,
        oracle.mismatches,
        if oracle.checks == 0 {
            " (set RSVELTE_INDEX_ORACLE to run it)"
        } else {
            ""
        }
    );
    println!(
        "  Template fragment:   {:7.2}ms ({:5.1}%)",
        ms(template_fragment),
        pct(template_fragment)
    );
    let tf_rest = ms(template_fragment)
        - [tf.clean, tf.template_str, tf.as_json, tf.parse]
            .iter()
            .copied()
            .map(ms)
            .sum::<f64>();
    for (label, val, calls) in [
        ("tf_clean", ms(tf.clean), tf.clean_calls),
        (
            "tf_template_str",
            ms(tf.template_str),
            tf.template_str_calls,
        ),
        ("tf_as_json", ms(tf.as_json), tf.as_json_calls),
        ("tf_parse", ms(tf.parse), tf.parse_calls),
        ("tf_rest (IR walk)", tf_rest, 0),
    ] {
        println!(
            "    {label:<18} {val:7.2}ms ({:5.1}% of tf) calls {calls}",
            val / ms(template_fragment) * 100.0
        );
    }
    // The pre-registered quantity: reparse + text splice + string assembly.
    // Splice has no term because there is no splice site under this visitor --
    // every `replace_range` in `3_transform/` is in the script pipeline.
    let tf_removable = ms(tf.parse) + ms(tf.template_str) + ms(tf.as_json);
    println!(
        "    REMOVABLE-IF-AST parse+templateStr+asJson {:.3}ms = {:.2}% of tf | without templateStr {:.2}%",
        tf_removable,
        tf_removable / ms(template_fragment) * 100.0,
        (ms(tf.parse) + ms(tf.as_json)) / ms(template_fragment) * 100.0
    );
    println!(
        "  Assembly (post-frag):{:7.2}ms ({:5.1}%)",
        ms(assembly_after),
        pct(assembly_after)
    );
    let as_rest = ms(assembly_after)
        - [asm.module_text, asm.css_inject, asm.as_json, asm.parse]
            .iter()
            .copied()
            .map(ms)
            .sum::<f64>();
    for (label, val, calls) in [
        ("as_module_text", ms(asm.module_text), asm.module_text_calls),
        ("as_css_inject", ms(asm.css_inject), asm.css_inject_calls),
        ("as_as_json", ms(asm.as_json), asm.as_json_calls),
        ("as_parse", ms(asm.parse), asm.parse_calls),
        ("as_rest (IR build)", as_rest, 0),
    ] {
        println!(
            "    {label:<18} {val:7.2}ms ({:5.1}% of as) calls {calls}",
            val / ms(assembly_after) * 100.0
        );
    }
    // `css_inject` is excluded: the stylesheet text is emitted output, so it is
    // built whatever the intermediate representation is.
    let as_removable = ms(asm.parse) + ms(asm.module_text) + ms(asm.as_json);
    println!(
        "    REMOVABLE-IF-AST parse+moduleText+asJson {:.3}ms = {:.2}% of as | with cssInject {:.2}%",
        as_removable,
        as_removable / ms(assembly_after) * 100.0,
        (as_removable + ms(asm.css_inject)) / ms(assembly_after) * 100.0
    );
    println!(
        "  CSS render:          {:7.2}ms ({:5.1}%)",
        ms(css_render),
        pct(css_render)
    );
    println!(
        "  JS codegen:          {:7.2}ms ({:5.1}%)",
        ms(codegen),
        pct(codegen)
    );
    println!(
        "  Pre-frag setup:      {:7.2}ms ({:5.1}%)",
        ms(other),
        pct(other)
    );
    let unattributed = pipeline.unattributed();
    println!(
        "Unattributed:          {:7.2}ms ({:5.1}%)",
        ms(unattributed),
        pct(unattributed)
    );
    println!(
        "TOTAL:                 {:7.2}ms  ({} compiles, {failed} failed)",
        ms(total),
        pipeline.compiles
    );
    println!();
    report_resolved(script_text, files.len());
    report_phase3_map(&scaling, rs);
    println!(
        "Per-file average:    {:.2}µs",
        total.as_secs_f64() * 1_000_000.0 / files.len() as f64
    );
    println!(
        "Throughput:          {:.1} MB/s",
        total_bytes as f64 / total.as_secs_f64() / 1_000_000.0
    );
}

/// Phase 3 shares on the shipped-source corpus, the three ways that answer
/// different questions.
///
/// The sum ratio is the one to pick targets with: it is the share of the
/// corpus's Phase 3 time a bucket owns, so a bucket that is large only because
/// one file is pathological still shows up. That is also why the other two
/// columns are printed beside it -- the per-file median says what a typical
/// file pays, and top1 says how much of the sum a single file contributed. When
/// the median is far below the sum ratio and top1 is large, the bucket is one
/// file wearing a mechanism's name.
fn report_phase3_map(rows: &[ScalingRow], rs: profile::ResidualBreakdown) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let buckets: [(&str, fn(&ScalingRow) -> std::time::Duration); 6] = [
        ("visitProgram", |r| r.visit_program),
        ("scriptText", |r| r.script_text),
        ("templateFragment", |r| r.template),
        ("assemblyAfterFrag", |r| r.assembly),
        ("cssRender", |r| r.css_render),
        ("codegen", |r| r.codegen),
    ];
    let parent: f64 = rows.iter().map(|r| ms(r.transform)).sum();
    // Files with no Phase 3 time have no share to take a median of, and the
    // same filter applies to every bucket so it cannot reorder them.
    let scored: Vec<&ScalingRow> = rows.iter().filter(|r| r.transform.as_nanos() > 0).collect();
    println!();
    println!("=== Phase 3 map (shipped corpus, sum ratio) ===");
    println!(
        "  {:<18} {:>10} {:>8} {:>10} {:>8}  n={}",
        "bucket",
        "sum ms",
        "sum%",
        "median%",
        "top1%",
        scored.len()
    );
    let mut sum_of_buckets = 0.0;
    for (label, get) in buckets {
        let vals: Vec<f64> = rows.iter().map(|r| ms(get(r))).collect();
        let sum: f64 = vals.iter().sum();
        sum_of_buckets += sum;
        let top1 = vals.iter().copied().fold(0.0f64, f64::max);
        let mut shares: Vec<f64> = scored
            .iter()
            .map(|r| ms(get(r)) / ms(r.transform) * 100.0)
            .collect();
        shares.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = if shares.is_empty() {
            0.0
        } else {
            shares[shares.len() / 2]
        };
        println!(
            "  {label:<18} {sum:>10.2} {:>7.1}% {median:>9.1}% {:>7.1}%",
            sum / parent * 100.0,
            if sum > 0.0 { top1 / sum * 100.0 } else { 0.0 }
        );
    }
    let residual = parent - sum_of_buckets;
    println!(
        "  {:<18} {residual:>10.2} {:>7.1}%",
        "unattributed",
        residual / parent * 100.0
    );
    let named: [(&str, std::time::Duration, u64); 5] = [
        ("rs_setup", rs.setup, rs.setup_calls),
        ("rs_shadow_fix", rs.shadow_fix, rs.shadow_fix_calls),
        ("rs_async_pre", rs.async_pre, rs.async_pre_calls),
        ("rs_warnings", rs.warnings, rs.warnings_calls),
        ("rs_sourcemap", rs.sourcemap, rs.sourcemap_calls),
    ];
    let mut named_sum = 0.0;
    for (label, d, calls) in named {
        let v = ms(d);
        named_sum += v;
        println!(
            "    {label:<16} {v:>10.2} {:>7.1}% of residual  calls {calls}",
            if residual > 0.0 {
                v / residual * 100.0
            } else {
                0.0
            }
        );
    }
    println!(
        "    {:<16} {:>10.2} {:>7.1}% of residual",
        "rs_rest",
        residual - named_sum,
        if residual > 0.0 {
            (residual - named_sum) / residual * 100.0
        } else {
            0.0
        }
    );
    // Printed as a line of its own because the buckets are drained from the same
    // per-file breakdown as the parent: a non-zero difference here is an
    // instrument fault, not a phase nobody named.
    println!(
        "  IDENTITY  Sum(buckets) {:.2} + residual {:.2} = {:.2} vs parent {:.2} (diff {:+.4}ms)",
        sum_of_buckets,
        residual,
        sum_of_buckets + residual,
        parent,
        sum_of_buckets + residual - parent
    );
}

/// Which arm of the rewrite driver answered, per pass.
///
/// `rescued` is the load-bearing column: those are the fragments the in-place
/// path declined and the text path then rewrote, so it is the count that says
/// whether deleting the text path would lose work. `neither` is the ordinary
/// case and carries no verdict -- most fragments have nothing for a given pass
/// to do -- and `text-pref` should be zero unless `RSVELTE_AST_SPLICE` is set,
/// which makes it the positive control for the whole table.
fn report_resolved(script_text: std::time::Duration, files: usize) {
    let rows = rsvelte_core::ast_rewrite_resolved_counts();
    let (in_place_ns, redundant_ns) = rsvelte_core::ast_rewrite_resolve_time();
    if rows.is_empty() {
        println!("resolve arms: no rows (the counters need the phase timers on)");
        return;
    }
    let (mut ip, mut resc, mut neither, mut pref) = (0u64, 0u64, 0u64, 0u64);
    println!("resolve arms by pass");
    println!(
        "  {:<34} {:>9} {:>9} {:>10} {:>10}",
        "pass", "in-place", "rescued", "neither", "text-pref"
    );
    let mut rows = rows;
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(u64::from(c[1])));
    for (pass, c) in &rows {
        ip += u64::from(c[0]);
        resc += u64::from(c[1]);
        neither += u64::from(c[2]);
        pref += u64::from(c[3]);
        println!(
            "  {:<34} {:>9} {:>9} {:>10} {:>10}",
            pass, c[0], c[1], c[2], c[3]
        );
    }
    println!(
        "  {:<34} {:>9} {:>9} {:>10} {:>10}",
        "TOTAL", ip, resc, neither, pref
    );
    let decided = ip + resc;
    if decided > 0 {
        println!(
            "  text path rescued {:.4}% of the rewrites that happened ({resc} of {decided})",
            resc as f64 / decided as f64 * 100.0
        );
    }
    let st_ns = script_text.as_nanos() as f64;
    println!(
        "  in-place half     {:8.3}ms  {:5.2}% of script_text   {:7.3}µs/file",
        in_place_ns as f64 / 1e6,
        in_place_ns as f64 / st_ns * 100.0,
        in_place_ns as f64 / 1e3 / files as f64
    );
    println!(
        "  text half (would vanish) {:8.3}ms  {:5.2}% of script_text   {:7.3}µs/file",
        redundant_ns as f64 / 1e6,
        redundant_ns as f64 / st_ns * 100.0,
        redundant_ns as f64 / 1e3 / files as f64
    );
}

/// Writes the per-file rows the scaling table is aggregated from.
///
/// Every refit -- excluding zero rows, restricting to a quartile range, fitting
/// all buckets on one common file set -- is a question about which rows enter
/// which fit, and none of them can be asked of the printed table. Dumping the
/// rows once makes those refits cost nothing.
///
/// A share taken from these rows needs the per-file median and the largest
/// row's share of its group printed beside it. One file here spends 492ms in
/// `script_text` -- a quarter of the whole corpus's transform -- so a bucket
/// share summed over a group is a statement about that file until the
/// concentration is shown to be low. The median said 15% where the sum said
/// 58%, from the same rows.
fn dump_rows(rows: &[ScalingRow], path: &str) {
    let ns = |d: std::time::Duration| d.as_nanos();
    let mut out = String::from(
        "file_bytes,script_bytes,runes,ensure_script,analyze,script_text,template,codegen,transform,reparse_bytes,reparse_calls,reparse_parse,direct_parse,visit_program,assembly,css_render\n",
    );
    for r in rows {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.file_bytes,
            r.script_bytes,
            r.runes,
            ns(r.ensure_script),
            ns(r.analyze),
            ns(r.script_text),
            ns(r.template),
            ns(r.codegen),
            ns(r.transform),
            r.reparse_bytes,
            r.reparse_calls,
            ns(r.reparse_parse),
            ns(r.direct_parse),
            ns(r.visit_program),
            ns(r.assembly),
            ns(r.css_render)
        );
    }
    match std::fs::write(path, out) {
        Ok(()) => println!("\n  wrote {} rows to {path}", rows.len()),
        Err(e) => eprintln!("could not write {path}: {e}"),
    }
}

struct ScalingRow {
    file_bytes: usize,
    script_bytes: usize,
    ensure_script: std::time::Duration,
    runes: usize,
    analyze: std::time::Duration,
    script_text: std::time::Duration,
    template: std::time::Duration,
    codegen: std::time::Duration,
    transform: std::time::Duration,
    reparse_bytes: u64,
    reparse_calls: u64,
    /// Time inside `Parser::parse` on the re-parse path, split by which entry
    /// reached the parser. The visitor half is deliberately absent: it means
    /// different things on the two halves of the choke point -- the mutable one
    /// prints inside it and the immutable one does not -- so a sum of the two
    /// has no single interpretation.
    reparse_parse: std::time::Duration,
    direct_parse: std::time::Duration,
    /// The three Phase 3 buckets the rows did not already carry, so the
    /// residual `transform - Σ(sub-buckets)` can be read per size bin instead
    /// of only as one corpus-wide figure.
    visit_program: std::time::Duration,
    assembly: std::time::Duration,
    css_render: std::time::Duration,
    /// Scan volume for this file alone. Carried per row because the question
    /// "does the pass count grow with script size" cannot be answered from a
    /// corpus-wide total, and no clock enters these three.
    scan_bytes: u64,
    scan_calls: u64,
    scan_staged_bytes: u64,
}

/// Script size and rune count for one file.
///
/// Runes are counted textually inside the script tags rather than taken from
/// the analysis, so the number means the same thing as the one a regression on
/// source text would produce. `$$props` and `$$restProps` are not runes and are
/// excluded by requiring a single leading `$`.
fn script_shape(ast: Option<&rsvelte_core::ast::Root<'_>>, content: &str) -> (usize, usize) {
    const RUNES: [&str; 7] = [
        "$state",
        "$derived",
        "$props",
        "$effect",
        "$bindable",
        "$inspect",
        "$host",
    ];
    let Some(ast) = ast else {
        return (0, 0);
    };
    let mut bytes = 0usize;
    let mut runes = 0usize;
    for script in [ast.instance.as_ref(), ast.module.as_ref()]
        .into_iter()
        .flatten()
    {
        let (start, end) = (script.start as usize, script.end as usize);
        let Some(text) = content.get(start..end) else {
            continue;
        };
        bytes += text.len();
        for rune in RUNES {
            let mut rest = text;
            while let Some(pos) = rest.find(rune) {
                let before_is_dollar = rest[..pos].ends_with('$');
                let after = &rest[pos + rune.len()..];
                // A rune is a call or a member access; `$stateful` is neither.
                let looks_like_rune = after.starts_with('(')
                    || after.starts_with('.')
                    || after.trim_start().starts_with('(');
                if !before_is_dollar && looks_like_rune {
                    runes += 1;
                }
                rest = &rest[pos + rune.len()..];
            }
        }
    }
    (bytes, runes)
}

/// Ordinary least squares slope of `log(y)` on `log(x)`, i.e. the exponent.
///
/// Rows where either side is zero carry no exponent information and are
/// dropped; the count that survived is reported so the slope is never read
/// without knowing what it was fitted on.
fn log_slope(points: &[(f64, f64)]) -> (f64, usize) {
    let used: Vec<(f64, f64)> = points
        .iter()
        .filter(|&&(x, y)| x > 0.0 && y > 0.0)
        .map(|&(x, y)| ((x + 1.0).ln(), y.ln()))
        .collect();
    let n = used.len();
    if n < 3 {
        return (f64::NAN, n);
    }
    let mx = used.iter().map(|p| p.0).sum::<f64>() / n as f64;
    let my = used.iter().map(|p| p.1).sum::<f64>() / n as f64;
    let num: f64 = used.iter().map(|&(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = used.iter().map(|&(x, _)| (x - mx) * (x - mx)).sum();
    (num / den, n)
}

/// Effective pass count per script-size band.
///
/// The band edges are the ones the wall-clock ns/B table already uses, so the
/// two can be read against each other: a flat ns/B and a growing pass count
/// cannot both be right. `impl ns/B` divides the measured 60 ns/B by the pass
/// count, which says what one pass would have to cost -- a number that can be
/// checked against what the code in that pass actually does.
fn report_scan_bands(rows: &[ScalingRow]) {
    const EDGES: [(usize, usize); 6] = [
        (0, 200),
        (200, 500),
        (500, 1000),
        (1000, 2000),
        (2000, 5000),
        (5000, usize::MAX),
    ];
    println!("\n  === scan volume by script size (deterministic) ===");
    println!("    band          n   script B    scan B   passes  staged  calls/f  impl ns/B");
    for (lo, hi) in EDGES {
        let band: Vec<&ScalingRow> = rows
            .iter()
            .filter(|r| r.script_bytes >= lo && r.script_bytes < hi && r.script_bytes > 0)
            .collect();
        if band.is_empty() {
            continue;
        }
        let script: u64 = band.iter().map(|r| r.script_bytes as u64).sum();
        let scan: u64 = band.iter().map(|r| r.scan_bytes).sum();
        let staged: u64 = band.iter().map(|r| r.scan_staged_bytes).sum();
        let passes = scan as f64 / script as f64;
        // Wall clock for the same band, so the two halves of
        // `time = passes x unit price` are printed side by side.
        let st: f64 = band.iter().map(|r| r.script_text.as_nanos() as f64).sum();
        let ns_per_byte = st / script as f64;
        let calls: u64 = band.iter().map(|r| r.scan_calls).sum();
        println!(
            "    {:>5}-{:<6} {:>5} {:>10} {:>9} {:>8.2} {:>7.2} {:>8.1} {:>10.2}",
            lo,
            if hi == usize::MAX { 0 } else { hi },
            band.len(),
            script,
            scan,
            passes,
            staged as f64 / script as f64,
            calls as f64 / band.len() as f64,
            if passes > 0.0 { ns_per_byte / passes } else { 0.0 }
        );
    }
    println!("    (last column = measured ns/B for this band / effective passes = what one pass costs)");
}

/// Bucket shares and scaling exponents against one predictor.
///
/// The claim this supports is "rsvelte's own scaling sits in bucket X". It is
/// not a claim about where the gap to another compiler sits: that would need
/// the other compiler's bucket split, which we do not have.
fn report_scaling(rows: &[ScalingRow], label: &str, predictor: fn(&ScalingRow) -> f64) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let buckets: [(&str, fn(&ScalingRow) -> std::time::Duration); 5] = [
        ("ensure_script", |r| r.ensure_script),
        ("Analyze", |r| r.analyze),
        ("script_text", |r| r.script_text),
        ("template", |r| r.template),
        ("js_codegen", |r| r.codegen),
    ];
    // Every bucket printed below has to be inside this, or the shares are not
    // parts of one whole: `ensure_script` sits outside `analyze + transform`, and
    // pricing it against that denominator pushed the column past 100%.
    let total_all: f64 = rows
        .iter()
        .map(|r| ms(r.ensure_script) + ms(r.analyze) + ms(r.transform))
        .sum();
    println!("\n  === scaling vs {label} (n = {}) ===", rows.len());

    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| predictor(&rows[a]).total_cmp(&predictor(&rows[b])));
    println!(
        "    {:<12} {:>8} {:>8} {:>8} {:>8} | {:>7} {:>7} {:>6} {:>6}",
        "bucket", "Q1 ms/f", "Q2 ms/f", "Q3 ms/f", "Q4 ms/f", "share", "exp", "c_b", "fitted"
    );
    // `log_slope` drops rows whose time is zero, so each bucket is fitted on its
    // own subpopulation; without this count the five exponents look commensurable.
    let mut c_sum = 0.0;
    let mut c_share_sum = 0.0;
    for (name, get) in buckets {
        let mut cells = [0.0f64; 4];
        for (q, cell) in cells.iter_mut().enumerate() {
            let chunk = &order[order.len() * q / 4..order.len() * (q + 1) / 4];
            *cell =
                chunk.iter().map(|&i| ms(get(&rows[i]))).sum::<f64>() / chunk.len().max(1) as f64;
        }
        let share = rows.iter().map(|r| ms(get(r))).sum::<f64>() / total_all.max(f64::MIN_POSITIVE);
        let pts: Vec<(f64, f64)> = rows.iter().map(|r| (predictor(r), ms(get(r)))).collect();
        let (exp, fitted) = log_slope(&pts);
        let c_b = share * exp;
        c_sum += c_b;
        c_share_sum += share;
        println!(
            "    {name:<12} {:>8.4} {:>8.4} {:>8.4} {:>8.4} | {:>6.1}% {:>7.3} {:>6.3} {:>6}",
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            share * 100.0,
            exp,
            c_b,
            fitted
        );
    }
    // The three transform sub-buckets do not cover `transform`; printing the
    // remainder is what lets the shares above be checked against 100% instead of
    // being read as a partition they are not.
    let uncovered: f64 = rows
        .iter()
        .map(|r| ms(r.transform) - ms(r.script_text) - ms(r.template) - ms(r.codegen))
        .sum::<f64>()
        / total_all.max(f64::MIN_POSITIVE);
    println!("    {:<12} {:>36} {:>6.1}%", "other", "", uncovered * 100.0);
    // A column that does not add to 100 invites the reading that the buckets
    // partition the total, which is how the shares were misread before.
    println!(
        "    {:<12} {:>36} {:>6.1}%",
        "SUM",
        "",
        (c_share_sum + uncovered) * 100.0
    );
    let total_pts: Vec<(f64, f64)> = rows
        .iter()
        .map(|r| {
            (
                predictor(r),
                ms(r.ensure_script) + ms(r.analyze) + ms(r.transform),
            )
        })
        .collect();
    let (total_exp, used) = log_slope(&total_pts);
    println!(
        "    SELF-CHECK  sum c_b {c_sum:.3} vs total exponent {total_exp:.3} (fitted on {used} of {})",
        rows.len()
    );
}

/// Re-parse cost overall and per file-size quartile.
///
/// The deterministic column is `bytes/file`: how many times over the pass
/// pipeline hands the same script back to the parser. It needs no quiet machine,
/// so it answers "constant factor or superlinear" independently of the timings
/// next to it.
fn report_reparse(
    rows: &mut [(usize, std::time::Duration, profile::ReparseBreakdown)],
    total_ms: f64,
) {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let sum: profile::ReparseBreakdown = rows.iter().fold(
        profile::ReparseBreakdown::default(),
        |mut acc, (_, _, r)| {
            acc.parse += r.parse;
            acc.visit += r.visit;
            acc.calls += r.calls;
            acc.bytes += r.bytes;
            acc.direct_parse += r.direct_parse;
            acc.direct_calls += r.direct_calls;
            acc.direct_bytes += r.direct_bytes;
            acc
        },
    );
    println!(
        "  reparse (driver):    {:7.2}ms ({:5.1}%) parse, {:7.2}ms ({:5.1}%) visit | {} calls",
        ms(sum.parse),
        ms(sum.parse) / total_ms * 100.0,
        ms(sum.visit),
        ms(sum.visit) / total_ms * 100.0,
        sum.calls
    );
    println!(
        "  reparse (direct):    {:7.2}ms ({:5.1}%) parse | {} calls, {} bytes",
        ms(sum.direct_parse),
        ms(sum.direct_parse) / total_ms * 100.0,
        sum.direct_calls,
        sum.direct_bytes
    );

    rows.sort_by_key(|&(bytes, ..)| bytes);
    let n = rows.len();
    if n < 4 {
        return;
    }
    println!(
        "    {:<9} {:>6} {:>9} {:>8} {:>10} {:>9} {:>9}",
        "quartile", "files", "med bytes", "calls/f", "reparse/f", "parse%P3", "visit%P3"
    );
    for q in 0..4 {
        let chunk = &rows[n * q / 4..n * (q + 1) / 4];
        let files = chunk.len() as f64;
        let src: u64 = chunk.iter().map(|&(b, ..)| b as u64).sum();
        let calls: u64 = chunk.iter().map(|(_, _, r)| r.calls + r.direct_calls).sum();
        let bytes: u64 = chunk.iter().map(|(_, _, r)| r.bytes + r.direct_bytes).sum();
        let parse: f64 = chunk
            .iter()
            .map(|(_, _, r)| ms(r.parse) + ms(r.direct_parse))
            .sum();
        let visit: f64 = chunk.iter().map(|(_, _, r)| ms(r.visit)).sum();
        let p3: f64 = chunk.iter().map(|&(_, d, _)| ms(d)).sum();
        println!(
            "    Q{:<8} {:>6} {:>9} {:>8.1} {:>9.2}x {:>8.1}% {:>8.1}%",
            q + 1,
            chunk.len(),
            chunk[chunk.len() / 2].0,
            calls as f64 / files,
            bytes as f64 / src.max(1) as f64,
            parse / p3.max(f64::MIN_POSITIVE) * 100.0,
            visit / p3.max(f64::MIN_POSITIVE) * 100.0,
        );
    }
}

/// The six shipped projects, picked so this population is byte-for-byte the one
/// the `$:` density check ran on: the density figure and the shares then
/// describe the same files rather than two similar-sounding sets.
const SHIPPED_PROJECTS: [&str; 6] = [
    "submodules/flowbite-svelte",
    "submodules/bits-ui",
    "submodules/shadcn-svelte",
    "submodules/layerchart",
    "submodules/skeleton",
    "submodules/svelte-ux",
];

fn collect_files() -> Vec<(String, String)> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if std::env::args().any(|a| a == "--shipped") {
        // `--only=a,b` / `--skip=a,b` narrow the set, so a share that turns out
        // to sit in one project can be attributed to it instead of guessed at.
        let list = |flag: &str| -> Vec<String> {
            std::env::args()
                .find_map(|a| a.strip_prefix(flag).map(str::to_owned))
                .map(|v| v.split(',').map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let only = list("--only=");
        let skip = list("--skip=");
        let mut files = Vec::new();
        for project in &SHIPPED_PROJECTS {
            let matches = |pats: &[String]| pats.iter().any(|p| project.contains(p.as_str()));
            if (!only.is_empty() && !matches(&only)) || matches(&skip) {
                continue;
            }
            collect_svelte_files(&base.join(project), &mut files);
        }
        return files;
    }
    let test_dir = base.join("submodules/svelte/packages/svelte/tests");
    let categories = [
        "parser-modern/samples",
        "snapshot/samples",
        "css/samples",
        "runtime-runes/samples",
        "runtime-legacy/samples",
        "runtime-browser/samples",
        "hydration/samples",
        "server-side-rendering/samples",
        "validator/samples",
    ];
    let mut files = Vec::new();
    for cat in &categories {
        let dir = test_dir.join(cat);
        if !dir.exists() {
            continue;
        }
        collect_svelte_files(&dir, &mut files);
    }
    files
}

fn collect_svelte_files(dir: &std::path::Path, files: &mut Vec<(String, String)>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_svelte_files(&path, files);
            } else if path.extension().is_some_and(|e| e == "svelte")
                && let Ok(content) = fs::read_to_string(&path)
            {
                files.push((path.display().to_string(), content));
            }
        }
    }
}
