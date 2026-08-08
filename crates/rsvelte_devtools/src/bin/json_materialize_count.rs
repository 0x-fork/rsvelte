//! Counts the `serde_json::Value` that a client compile materializes out of the
//! typed `JsNode` AST, over the flowbite-svelte corpus.
//!
//! Load-independent alternative to a sampling profile: the JSON-backed readers
//! in Phase 3 are all read-only queries, so every object and key counted here is
//! work that a typed reader would not do. Requires the instrumentation feature:
//!
//! ```text
//! cargo run --profile profiling -p rsvelte_devtools --bin json_materialize_count \
//!   --features measure-json
//! ```

#[cfg(feature = "measure-json")]
use std::fs;
#[cfg(feature = "measure-json")]
use std::path::{Path, PathBuf};

#[cfg(feature = "measure-json")]
use rsvelte_core::{CompileOptions, GenerateMode, compile};

#[cfg(feature = "measure-json")]
fn collect(dir: &Path, files: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            collect(&path, files);
        } else if path.extension().is_some_and(|e| e == "svelte")
            && let Ok(content) = fs::read_to_string(&path)
        {
            files.push((path, content));
        }
    }
}

#[cfg(not(feature = "measure-json"))]
fn main() {
    eprintln!("build with --features measure-json");
    std::process::exit(2);
}

#[cfg(feature = "measure-json")]
fn main() {
    use rsvelte_core::ast::js::measure_json;

    let mut mode = GenerateMode::Client;
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("submodules/flowbite-svelte");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dev = args.iter().any(|a| a == "--dev");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dev" => {}
            "--mode" => {
                i += 1;
                mode = match args[i].as_str() {
                    "server" => GenerateMode::Server,
                    _ => GenerateMode::Client,
                };
            }
            "--dir" => {
                i += 1;
                root = PathBuf::from(&args[i]);
            }
            other => panic!("unknown argument: {other}"),
        }
        i += 1;
    }

    let mut files = Vec::new();
    collect(&root, &mut files);
    assert!(
        !files.is_empty(),
        "no .svelte files under {}",
        root.display()
    );

    // Warm up so lazily-built statics are not charged to the compile clock.
    for (_, content) in files.iter().take(100) {
        let _ = compile(
            content,
            CompileOptions {
                generate: mode,
                dev,
                ..Default::default()
            },
        );
    }

    measure_json::reset();
    let t0 = std::time::Instant::now();
    for (_, content) in &files {
        let _ = compile(
            content,
            CompileOptions {
                generate: mode,
                dev,
                ..Default::default()
            },
        );
    }
    let compile_ns = t0.elapsed().as_nanos() as u64;
    let (materializations, objects, entries, strings) = measure_json::snapshot();
    let to_value_ns = measure_json::to_value_ns();

    let n = files.len() as f64;
    println!("files: {}", files.len());
    println!(
        "materializations: {materializations} total, {:.1}/file",
        materializations as f64 / n
    );
    println!(
        "objects:          {objects} total, {:.1}/file",
        objects as f64 / n
    );
    println!(
        "map entries:      {entries} total, {:.1}/file  (= key String allocs = map inserts = hashes)",
        entries as f64 / n
    );
    println!(
        "strings:          {strings} total, {:.1}/file  (keys + string values)",
        strings as f64 / n
    );
    println!("\nget_literal_value root node types:");
    for (kind, count) in measure_json::kinds() {
        println!("  {count:7}  {kind}");
    }
    let (all_ns, all_calls) = measure_json::all_to_value();
    println!(
        "\nto_value via the lazy cache (1 of 54 sites): {:.1}ms of {:.1}ms compile  ({:.2}%)",
        to_value_ns as f64 / 1e6,
        compile_ns as f64 / 1e6,
        to_value_ns as f64 / compile_ns as f64 * 100.0
    );
    println!(
        "to_value, ALL 54 sites:                      {:.1}ms of {:.1}ms compile  ({:.2}%), {all_calls} calls",
        all_ns as f64 / 1e6,
        compile_ns as f64 / 1e6,
        all_ns as f64 / compile_ns as f64 * 100.0
    );
    println!(
        "  → direct (cache-bypassing) share of to_value: {:.1}% of calls, {:.1}% of time",
        (all_calls.saturating_sub(materializations)) as f64 / all_calls.max(1) as f64 * 100.0,
        (all_ns.saturating_sub(to_value_ns)) as f64 / all_ns.max(1) as f64 * 100.0
    );
    println!("\nto_value by call site (calls, objects, map entries):");
    for (site, calls, objects, entries) in measure_json::sites() {
        println!("  {calls:6}  {objects:8}  {entries:8}  {site}");
    }
    println!("\nby caller (materializations, objects):");
    for (site, count, objects) in measure_json::callers() {
        println!(
            "  {count:7} {:5.1}%  {objects:9}  {site}",
            count as f64 / materializations.max(1) as f64 * 100.0
        );
    }
}
