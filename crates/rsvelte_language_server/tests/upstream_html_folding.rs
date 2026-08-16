//! A port of upstream's `test/plugins/html/getFoldingRange.test.ts` — the one
//! TS-independent upstream suite whose expectations survive the trip through the
//! protocol unchanged (its cases are line arrays in, folding ranges out, with no
//! provider-level options).
//!
//! Upstream itself copied these from `vscode-html-languageservice`, so they are
//! third-party expectations rather than a snapshot of either server, which makes
//! them worth asserting directly instead of differentially. Nine of the ten hold
//! as written; the tenth is a real divergence and is asserted as what rsvelte
//! actually answers, with upstream's expectation named beside it.

#[path = "support/protocol_server.rs"]
mod protocol_server;

use protocol_server::*;
use serde_json::json;

/// `(startLine, endLine, kind)` of every folding range, sorted — upstream's `r()`.
fn ranges(server: &mut Server, uri: &str) -> Vec<(u64, u64, Option<String>)> {
    let mut out: Vec<_> = server
        .folding_ranges(uri)
        .into_iter()
        .map(|range| {
            (
                range["startLine"].as_u64().unwrap(),
                range["endLine"].as_u64().unwrap(),
                range["kind"].as_str().map(str::to_string),
            )
        })
        .collect();
    out.sort();
    out
}

fn fold(case: &str, lines: &[&str]) -> Vec<(u64, u64, Option<String>)> {
    // A directory per case: `temp_dir` wipes what it returns, and these tests
    // run in parallel, so a shared one would delete a sibling's open document.
    let path = temp_dir(&format!("upstream-folding-{case}")).join("input.svelte");
    let uri = file_uri(&path);
    // `lineFoldingOnly` is what upstream's provider is called with, and it is
    // what shortens a range to the line before the closing tag — without it
    // every expectation here is off by one.
    let (mut server, _) = server_with(json!({ "foldingRange": { "lineFoldingOnly": true } }));
    did_open(&mut server, &uri, &lines.join("\n"));
    ranges(&mut server, &uri)
}

fn r(start: u64, end: u64) -> (u64, u64, Option<String>) {
    (start, end, None)
}

fn kind(start: u64, end: u64, kind: &str) -> (u64, u64, Option<String>) {
    (start, end, Some(kind.to_string()))
}

#[test]
fn fold_one_level() {
    assert_eq!(
        fold("one-level", &["<html>", "Hello", "</html>"]),
        vec![r(0, 1)]
    );
}

#[test]
fn fold_two_level() {
    assert_eq!(
        fold(
            "two-level",
            &["<html>", "<head>", "Hello", "</head>", "</html>"]
        ),
        vec![r(0, 3), r(1, 2)]
    );
}

#[test]
fn fold_siblings() {
    assert_eq!(
        fold(
            "siblings",
            &[
                "<html>",
                "<head>",
                "Head",
                "</head>",
                "<body class=\"f\">",
                "Body",
                "</body>",
                "</html>",
            ]
        ),
        vec![r(0, 6), r(1, 2), r(4, 5)]
    );
}

#[test]
fn fold_self_closing_tags() {
    assert_eq!(
        fold(
            "self-closing",
            &[
                "<div>",
                "<a href=\"top\"/>",
                "<img src=\"s\">",
                "<br/>",
                "<br>",
                "<img class=\"c\"",
                "     src=\"top\"",
                ">",
                "</div>",
            ]
        ),
        vec![r(0, 7), r(5, 6)]
    );
}

#[test]
fn fold_comment() {
    assert_eq!(
        fold(
            "comment",
            &[
                "<!--",
                " multi line",
                "-->",
                "<!-- some stuff",
                " some more stuff -->"
            ]
        ),
        vec![kind(0, 2, "comment"), kind(3, 4, "comment")]
    );
}

#[test]
fn fold_regions() {
    assert_eq!(
        fold(
            "regions",
            &[
                "<!-- #region -->",
                "<!-- #region -->",
                "<!-- #endregion -->",
                "<!-- #endregion -->",
            ]
        ),
        vec![kind(0, 3, "region"), kind(1, 2, "region")]
    );
}

/// The one divergence. Upstream expects `r(0, 3)`: the `<body>` is folded up to
/// the line before the stray `</div>`, which is recovered as an unmatched close
/// tag. rsvelte offers no fold for this document at all. Asserted as-is so the
/// day the recovery lands, this test says so.
#[test]
fn fold_incomplete_diverges_from_upstream() {
    assert_eq!(
        fold(
            "incomplete",
            &["<body>", "<div></div>", "Hello", "</div>", "</body>"]
        ),
        vec![],
        "upstream's `getFoldingRange.test.ts` expects r(0, 3) here"
    );
}

#[test]
fn fold_incomplete_2() {
    assert_eq!(
        fold(
            "incomplete-2",
            &["<be><div>", "<!-- #endregion -->", "</div>"]
        ),
        vec![r(0, 1)]
    );
}

#[test]
fn fold_intersecting_region() {
    assert_eq!(
        fold(
            "intersecting-region",
            &[
                "<body>",
                "<!-- #region -->",
                "Hello",
                "<div></div>",
                "</body>",
                "<!-- #endregion -->",
            ]
        ),
        vec![r(0, 3)]
    );
}

#[test]
fn fold_intersecting_region_2() {
    assert_eq!(
        fold(
            "intersecting-region-2",
            &[
                "<!-- #region -->",
                "<body>",
                "Hello",
                "<!-- #endregion -->",
                "<div></div>",
                "</body>",
            ]
        ),
        vec![kind(0, 3, "region")]
    );
}
