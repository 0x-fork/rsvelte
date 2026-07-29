//! Stable svelte2tsx benchmarks for Criterion and CodSpeed.

#[cfg(all(
    feature = "mimalloc-alloc",
    not(target_arch = "wasm32"),
    not(target_os = "windows")
))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rsvelte_projection::svelte2tsx::RewriteExternalImportsOptions;
use rsvelte_projection::svelte2tsx::{Svelte2TsxOptions, svelte2tsx};
use std::fmt::Write as _;
use std::hint::black_box;

#[path = "common/corpus.rs"]
mod corpus;
use corpus::Sample;

fn create_script_heavy_file() -> Sample {
    Sample::synthetic(
        "synthetic-script-heavy",
        r#"<script module lang="ts">
    import { readable } from 'svelte/store';
    export interface Shared { id: number; label: string }
    export const moduleStore = readable<Shared>({ id: 1, label: 'one' });
</script>

<script lang="ts" generics="T extends Shared">
    import type { Component } from 'svelte';
    import { writable } from 'svelte/store';
    export let value: T;
    export let renderer: Component<{ value: T }>;
    const resolved = await Promise.resolve(value);
    const local = writable(resolved);
    $: current = $local;
</script>

<svelte:component this={renderer} value={current} />
<p>{current.label}</p>
"#
        .to_string(),
    )
}

fn create_source_map_heavy_file() -> Sample {
    let mut source = String::from(
        r#"<script lang="ts">
    let selected = $state(0);
    const rows: Array<{ id: number; label: string }> = [];
</script>

"#,
    );
    for i in 0..100 {
        let _ = writeln!(
            source,
            r#"<button class:active={{selected === {i}}} onclick={{() => selected = {i}}}>{{rows[{i}]?.label ?? "row {i}"}}</button>"#
        );
    }
    Sample::synthetic("synthetic-source-map-heavy", source)
}

fn create_named_slots_file() -> Sample {
    let mut source = String::from("<Component let:item>\n");
    for i in 0..128 {
        let _ = writeln!(source, r#"    <div slot="slot-{i}">{{item}}</div>"#);
    }
    source.push_str("</Component>\n");
    Sample::synthetic("synthetic-named-slots", source)
}

fn workload() -> Vec<Sample> {
    let mut files = corpus::load();
    files.push(Sample::synthetic(
        "synthetic-no-script",
        "<h1>Hello, {name}</h1>\n{#if ready}<slot />{/if}\n".to_string(),
    ));
    files.push(Sample::synthetic(
        "synthetic-module-only",
        r#"<script module lang="ts">
    import { readable } from 'svelte/store';
    export const count = readable(1);
</script>

<p>{$count}</p>
"#
        .to_string(),
    ));
    files.push(create_script_heavy_file());
    files.push(create_source_map_heavy_file());
    files.push(create_named_slots_file());
    files
}

fn options(sample: &Sample) -> Svelte2TsxOptions {
    Svelte2TsxOptions {
        filename: format!("{}.svelte", sample.id),
        is_ts_file: sample.source.contains("lang=\"ts\""),
        ..Default::default()
    }
}

fn rewrite_options(sample: &Sample) -> Svelte2TsxOptions {
    Svelte2TsxOptions {
        rewrite_external_imports: Some(RewriteExternalImportsOptions {
            source_path: "/workspace/src/routes/Component.svelte".to_string(),
            generated_path: "/workspace/.generated/src/routes/Component.svelte.tsx".to_string(),
            workspace_path: "/workspace".to_string(),
        }),
        ..options(sample)
    }
}

fn bench_files(c: &mut Criterion) {
    let files = workload();
    let mut group = c.benchmark_group("svelte2tsx");

    for sample in &files {
        svelte2tsx(&sample.source, options(sample))
            .unwrap_or_else(|error| panic!("bench sample `{}` failed: {error}", sample.id));
        group.throughput(Throughput::Bytes(sample.bytes()));
        group.bench_with_input(BenchmarkId::new("file", &sample.id), sample, |b, sample| {
            b.iter(|| {
                svelte2tsx(black_box(&sample.source), black_box(options(sample)))
                    .expect("validated benchmark input")
            });
        });
    }

    group.finish();
}

fn bench_corpus(c: &mut Criterion) {
    let files = workload();
    let total_bytes = files.iter().map(Sample::bytes).sum();
    let mut group = c.benchmark_group("svelte2tsx_corpus");
    group.throughput(Throughput::Bytes(total_bytes));
    group.bench_function("single_thread", |b| {
        b.iter(|| {
            for sample in &files {
                black_box(
                    svelte2tsx(black_box(&sample.source), black_box(options(sample)))
                        .expect("validated benchmark input"),
                );
            }
        });
    });
    group.finish();
}

fn bench_external_import_rewrites(c: &mut Criterion) {
    let samples = [
        Sample::synthetic(
            "no-op",
            r#"<script lang="ts">
    import type { Shared } from '../shared.js';
    export let value: Shared;
</script>

<p>{value}</p>
"#
            .to_string(),
        ),
        Sample::synthetic(
            "rewritten",
            r#"<script lang="ts">
    import type { External } from '../../../outside.js';
    export let value: External;
</script>

<p>{value}</p>
"#
            .to_string(),
        ),
    ];
    let mut group = c.benchmark_group("svelte2tsx_external_imports");

    for sample in &samples {
        svelte2tsx(&sample.source, rewrite_options(sample))
            .unwrap_or_else(|error| panic!("bench sample `{}` failed: {error}", sample.id));
        group.throughput(Throughput::Bytes(sample.bytes()));
        group.bench_with_input(
            BenchmarkId::from_parameter(&sample.id),
            sample,
            |b, sample| {
                b.iter(|| {
                    svelte2tsx(
                        black_box(&sample.source),
                        black_box(rewrite_options(sample)),
                    )
                    .expect("validated benchmark input")
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_files,
    bench_corpus,
    bench_external_import_rewrites
);
criterion_main!(benches);
