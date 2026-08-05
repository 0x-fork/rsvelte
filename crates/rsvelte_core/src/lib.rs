//! Low-level compiler implementation for the rsvelte toolchain.
//!
//! Most embedders should use the compiler-neutral [`rsvelte`](https://docs.rs/rsvelte)
//! facade. This crate exposes rsvelte's parser, AST, phases, and raw compiler
//! options for workspace products and advanced integrations. Those low-level
//! types follow a pre-1.0 compatibility policy and may change between minor
//! releases as Svelte compatibility work evolves.
//!
//! The default feature set is empty. `parallel` adds only batch/parser
//! parallelism; host policy such as filesystem access, watching, CLI parsing,
//! allocator selection, and language bindings belongs to dedicated crates.
//!
//! # Low-level usage
//!
//! ```rust,no_run
//! use rsvelte_core::{Allocator, parse, ParseOptions};
//!
//! let source = r#"<h1>Hello, {name}!</h1>"#;
//! let allocator = Allocator::default();
//! let ast = parse(source, &allocator, ParseOptions::default()).unwrap();
//! ```

// `#[global_allocator]` deliberately lives only in artifacts that explicitly
// own allocator policy (for example repository profiling binaries and the
// `rsvelte_napi` cdylib), never in this library. Ordinary CLIs inherit the
// platform allocator, and embedding this rlib never imposes one on the host.

pub mod ast;
pub mod compiler;
pub mod error;
#[cfg(feature = "measure-ast-state")]
pub mod measure_ast_state;
#[cfg(feature = "measure-await")]
pub mod measure_await;
#[cfg(feature = "measure-hoisted")]
pub mod measure_hoisted;
#[cfg(feature = "measure-module-source")]
pub mod measure_module_source;
#[cfg(feature = "measure-prop-reads")]
pub mod measure_prop_reads;
#[cfg(feature = "measure-slot-key")]
pub mod measure_slot_key;
pub mod toolchain;

pub use compiler::legacy::convert_to_legacy;
#[cfg(not(feature = "parallel"))]
pub use compiler::phases::phase1_parse::{ParseOptions, parse};
#[cfg(feature = "parallel")]
pub use compiler::phases::phase1_parse::{ParseOptions, parse, parse_parallel};
pub use compiler::print::{PrintError, PrintOptions, PrintResult, print};

/// `(pass, runs, raw diffs, mismatches, unverified)` from the
/// `RSVELTE_AST_DUAL_RUN` equivalence harness, for the migration of the Phase-3
/// rewrite passes off text splicing. Exposed so a corpus driver can report the
/// tally; not part of the public API.
#[doc(hidden)]
pub fn ast_rewrite_dual_run_tally() -> Vec<(&'static str, u32, u32, u32, u32)> {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::tally()
}

/// How many times a Phase-3 rewrite pass re-parsed an intermediate script.
#[doc(hidden)]
pub fn ast_rewrite_dual_run_parses() -> u32 {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::parses()
}

/// `(pass, re-parses)` — which Phase-3 rewrite passes actually run.
#[doc(hidden)]
pub fn ast_rewrite_dual_run_parses_by_pass() -> Vec<(&'static str, u32)> {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::parses_by_pass()
}

/// Per-pass work counters, split into what the text path did and what the
/// in-place path did. Load-independent, so a port's effect is decided by
/// counting instead of by timing runs that differ by less than the noise.
#[doc(hidden)]
pub use compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::Work as AstRewriteWork;

/// `(pass, text-path work, in-place work)`.
#[doc(hidden)]
pub fn ast_rewrite_dual_run_work() -> Vec<(&'static str, AstRewriteWork, AstRewriteWork)> {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::work()
}

/// `(pass, [in-place, text-rescued, neither rewrote, text preferred])`, cleared.
///
/// Counted in the shipped configuration rather than under the equivalence
/// harness, because the harness runs both paths by construction and so cannot
/// say which one the compiler would have used. `text-rescued` is the column
/// that decides whether the text path can be deleted: it counts the fragments
/// the in-place path declined and the text path then rewrote. Requires the
/// phase timers to be on.
#[doc(hidden)]
pub fn ast_rewrite_resolved_counts() -> Vec<(&'static str, [u32; 4])> {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::take_resolved()
}

/// `(in-place nanos, redundant-text nanos)` for the ported Phase-3 passes.
///
/// The second number is what deleting the text path would give back, and only
/// that: on the arm where in-place rewrites the fragment the text half never
/// runs, so its cost there is already zero. Requires the phase timers to be on.
#[doc(hidden)]
pub fn ast_rewrite_resolve_time() -> (u64, u64) {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::take_resolve_time()
}

/// `(terminators dropped, of those the ones the gate could not check)` for the
/// Phase-3 in-place path. A fragment that does not stand alone parses to `None`
/// on both sides, which the gate reads as agreement, so the second number is
/// the part of the denominator nothing verified.
#[doc(hidden)]
pub fn ast_rewrite_termination_counts() -> (u32, u32) {
    compiler::phases::phase3_transform::shared::ast_rewrite::dual_run::termination_counts()
}
#[cfg(feature = "parallel")]
pub use compiler::{
    CompileError, CompileOptions, CompileResult, CssMode, ExperimentalOptions, GenerateMode,
    ModuleCompileOptions, Warning, WarningFilterFn, compile, compile_batch, compile_both,
    compile_module,
};
#[cfg(not(feature = "parallel"))]
pub use compiler::{
    CompileError, CompileOptions, CompileResult, CssMode, ExperimentalOptions, GenerateMode,
    ModuleCompileOptions, Warning, WarningFilterFn, compile, compile_both, compile_module,
};
#[doc(hidden)]
pub use oxc_allocator::Allocator;
