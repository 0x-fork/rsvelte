//! A warning's `start` is what an editor or CLI turns into a squiggle, so it is
//! part of the contract even though the message text is not. Upstream passes the
//! node to warn on as the first argument of `w.<code>(node, …)`; several rsvelte
//! emission sites dropped it and produced a warning with no span at all, which
//! leaves consumers nothing to point at.
//!
//! Each test below pins the position against the node upstream names, not merely
//! that the warning fires — the codes were already correct.

use rsvelte_core::{CompileOptions, GenerateMode, Warning, compile};

fn warnings(src: &str) -> Vec<Warning> {
    compile(
        src,
        CompileOptions {
            filename: Some("Main.svelte".to_string()),
            generate: GenerateMode::Client,
            dev: false,
            ..Default::default()
        },
    )
    .expect("compile")
    .warnings
}

/// `(line, column)` of the nth warning with `code`, plus the source text it
/// points at. Panics with the observed codes when the warning is missing, and
/// distinctly when it carries no position — the two are different defects.
fn at<'a>(src: &'a str, ws: &'a [Warning], code: &str, nth: usize) -> (usize, usize, &'a str) {
    let w = ws
        .iter()
        .filter(|w| w.code == code)
        .nth(nth)
        .unwrap_or_else(|| {
            panic!(
                "no `{code}` warning #{nth} in {:?}",
                ws.iter().map(|w| &w.code).collect::<Vec<_>>()
            )
        });
    let pos = w
        .start
        .as_ref()
        .unwrap_or_else(|| panic!("`{code}` has no start position"));
    let line = src.lines().nth(pos.line - 1).unwrap_or("");
    (pos.line, pos.column, &line[pos.column.min(line.len())..])
}

// ---- event_directive_deprecated -------------------------------------------
// Upstream: `visitors/OnDirective.js` -> `w.event_directive_deprecated(node, node.name)`,
// where `node` is the OnDirective itself.

const EVENT_DIRECTIVE: &str = "event_directive_deprecated";

#[test]
fn event_directive_points_at_the_directive_not_the_element() {
    let src = "<script>\n\tlet count = $state(0);\n</script>\n\n<button class=\"x\" on:click={() => count++}>{count}</button>\n";
    let ws = warnings(src);
    let (line, column, text) = at(src, &ws, EVENT_DIRECTIVE, 0);
    assert_eq!(line, 5, "expected the line holding the directive");
    assert!(
        text.starts_with("on:click={"),
        "expected the directive, got {line}:{column} -> {text:?}"
    );
}

/// Two directives on one element must get distinct columns, so the position
/// cannot be a per-element constant.
#[test]
fn event_directive_each_directive_gets_its_own_position() {
    let src = "<script>\n\tlet count = $state(0);\n</script>\n\n<button on:click={() => count++} on:mouseenter={() => count++}>{count}</button>\n";
    let ws = warnings(src);
    let (_, first, first_text) = at(src, &ws, EVENT_DIRECTIVE, 0);
    let (_, second, second_text) = at(src, &ws, EVENT_DIRECTIVE, 1);
    assert!(first_text.starts_with("on:click="), "got {first_text:?}");
    assert!(
        second_text.starts_with("on:mouseenter="),
        "got {second_text:?}"
    );
    assert!(first < second);
}

// ---- element_invalid_self_closing_tag --------------------------------------
// Upstream: `visitors/RegularElement.js` -> `w.element_invalid_self_closing_tag(node, node.name)`,
// where `node` is the element.

const SELF_CLOSING: &str = "element_invalid_self_closing_tag";

#[test]
fn self_closing_points_at_the_offending_element() {
    let src = "<div>\n\t<span />\n</div>\n";
    let ws = warnings(src);
    let (line, column, text) = at(src, &ws, SELF_CLOSING, 0);
    assert_eq!(line, 2, "expected the line holding the self-closing tag");
    assert!(
        text.starts_with("<span />"),
        "expected the element, got {line}:{column} -> {text:?}"
    );
}

/// Two offenders on one line must get distinct columns.
#[test]
fn self_closing_each_element_gets_its_own_position() {
    let src = "<p />, <b />\n";
    let ws = warnings(src);
    let (_, first, first_text) = at(src, &ws, SELF_CLOSING, 0);
    let (_, second, second_text) = at(src, &ws, SELF_CLOSING, 1);
    assert_eq!(first, 0, "expected `<p />` at column 0");
    assert!(first_text.starts_with("<p />"));
    assert_eq!(second, 7, "expected `<b />` at column 7");
    assert!(second_text.starts_with("<b />"));
}

/// Void and SVG elements are exempt — pins that attaching a span did not widen
/// which elements warn.
#[test]
fn self_closing_void_and_svg_still_do_not_warn() {
    let ws = warnings("<br />\n<svg><rect /></svg>\n");
    assert!(
        !ws.iter().any(|w| w.code == SELF_CLOSING),
        "unexpected `{SELF_CLOSING}` in {:?}",
        ws.iter().map(|w| &w.code).collect::<Vec<_>>()
    );
}

// ---- export_let_unused -----------------------------------------------------
// Upstream: `2-analyze/index.js` -> `w.export_let_unused(binding.node, name)`,
// where `binding.node` is the declaration identifier.

const EXPORT_LET_UNUSED: &str = "export_let_unused";

#[test]
fn export_let_unused_points_at_the_declared_identifier() {
    let src = "<script>\n\texport let unused;\n</script>\n\n<p>hi</p>\n";
    let ws = warnings(src);
    let (line, column, text) = at(src, &ws, EXPORT_LET_UNUSED, 0);
    assert_eq!(line, 2, "expected the line holding the declaration");
    assert!(
        text.starts_with("unused;"),
        "expected the identifier, not the statement, got {line}:{column} -> {text:?}"
    );
}

/// Two unused props declared on one line must get distinct columns.
#[test]
fn export_let_unused_each_declaration_gets_its_own_position() {
    let src = "<script>\n\texport let first, second;\n</script>\n\n<p>hi</p>\n";
    let ws = warnings(src);
    let cols: Vec<usize> = ws
        .iter()
        .filter(|w| w.code == EXPORT_LET_UNUSED)
        .map(|w| {
            w.start
                .as_ref()
                .unwrap_or_else(|| panic!("`{EXPORT_LET_UNUSED}` has no start position"))
                .column
        })
        .collect();
    assert_eq!(cols.len(), 2, "expected one warning per declarator");
    let line = src.lines().nth(1).unwrap();
    let mut sorted = cols.clone();
    sorted.sort_unstable();
    assert!(line[sorted[0]..].starts_with("first,"));
    assert!(line[sorted[1]..].starts_with("second;"));
}
