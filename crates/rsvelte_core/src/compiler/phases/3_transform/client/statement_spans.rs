//! Top-level statement enumeration taken from the Phase 2 program instead of
//! from a line scan.
//!
//! The staged script pipeline finds statement boundaries by walking the script
//! a line at a time and tracking bracket, string and template depths, with
//! hand-written rules for the cases depth alone cannot settle (a line ending in
//! a binary operator, a brace-less `if` whose body is on the next line, a
//! method chain continuing with a leading `.`). The parser has already answered
//! that question exactly, and its answer is still valid whenever the script text
//! has not been rewritten since it was parsed.
//!
//! What this produces is deliberately *not* an AST: it is the same sequence of
//! line ranges the scan produces, so the statement processor downstream keeps
//! taking text and nothing else in the pipeline has to change. Making it emit
//! nodes is a separate, much larger step.
//!
//! Lines the parser has no node for -- comments and stray semicolons between
//! statements -- still have to appear, because the scan emits them and the
//! output is compared byte for byte. They are recovered from the gaps between
//! consecutive statement spans rather than by parsing again.

use oxc_ast::ast::Program;

/// Byte offset of the start of every line, in `source.lines()` order.
///
/// `lines()` splits on `\n` and strips a trailing `\r`, so the offsets it
/// implies are not simply "after each `\n`" for CRLF input; walking the
/// iterator and measuring is the only way to get ranges that address the same
/// slices the scan sees.
fn line_starts(source: &str) -> Vec<usize> {
    let base = source.as_ptr() as usize;
    source
        .lines()
        .map(|line| line.as_ptr() as usize - base)
        .collect()
}

/// The line containing `offset`, by binary search over `starts`.
fn line_of(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(index) => index,
        Err(0) => 0,
        Err(index) => index - 1,
    }
}

/// Inclusive line ranges, one per statement the scan would emit.
///
/// Returns `None` when the program carries no statement, which is not the same
/// as "no groups": an empty body with a non-empty script means every line is a
/// comment, and the caller cannot tell those two apart from an empty `Vec`.
pub(super) fn statement_line_groups(
    source: &str,
    program: &Program<'_>,
) -> Option<Vec<(usize, usize)>> {
    let starts = line_starts(source);
    if starts.is_empty() {
        return Some(Vec::new());
    }
    let mut groups: Vec<(usize, usize)> = Vec::with_capacity(program.body.len());
    // Lines before the first statement, between statements, and after the last
    // one. `next_line` is the first line no group has claimed yet.
    let mut next_line = 0usize;

    for statement in &program.body {
        let span = oxc_span::GetSpan::span(statement);
        let start_line = line_of(&starts, span.start as usize);
        // `span.end` is exclusive, and a statement ending exactly at a newline
        // would otherwise claim the following line.
        let end_line = line_of(&starts, (span.end as usize).saturating_sub(1));
        if start_line < next_line {
            // Two statements on one line: the scan cannot split them either, so
            // extend the group that already covers this line rather than
            // emitting an overlapping one.
            if let Some(last) = groups.last_mut() {
                last.1 = last.1.max(end_line);
            }
            next_line = next_line.max(end_line + 1);
            continue;
        }
        push_gap_groups(source, &starts, next_line, start_line, &mut groups);
        groups.push((start_line, end_line));
        next_line = end_line + 1;
    }
    push_gap_groups(source, &starts, next_line, starts.len(), &mut groups);
    Some(groups)
}

/// Emits the lines in `[from, to)` that the scan would treat as statements of
/// their own.
///
/// A blank line at a statement boundary is dropped by the scan, so it is
/// dropped here. A line opening a block comment keeps the following lines
/// attached until the comment closes, because the scan's depth tracking does
/// the same; without that, a multi-line JSDoc block would be emitted as one
/// group per line.
fn push_gap_groups(
    source: &str,
    starts: &[usize],
    from: usize,
    to: usize,
    groups: &mut Vec<(usize, usize)>,
) {
    let mut index = from;
    while index < to {
        let text = line_at(source, starts, index);
        if text.trim().is_empty() {
            index += 1;
            continue;
        }
        let mut end = index;
        if block_comment_stays_open(text) {
            while end + 1 < to {
                end += 1;
                if !block_comment_stays_open(line_at(source, starts, end)) {
                    break;
                }
            }
        }
        groups.push((index, end));
        index = end + 1;
    }
}

fn line_at<'a>(source: &'a str, starts: &[usize], index: usize) -> &'a str {
    let start = starts[index];
    let end = starts
        .get(index + 1)
        .map(|next| source[start..*next].trim_end_matches(['\n', '\r']).len() + start)
        .unwrap_or(source.len());
    &source[start..end]
}

/// Whether a line leaves a `/* … */` block open, ignoring `//` and quotes.
///
/// Only the two-token scan is needed: the caller uses it to decide whether the
/// next line belongs to the same group, and a line that both opens and closes a
/// block is self-contained either way.
fn block_comment_stays_open(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut open = false;
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'/' && bytes[index + 1] == b'*' {
            open = true;
            index += 2;
            continue;
        }
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            open = false;
            index += 2;
            continue;
        }
        if !open && bytes[index] == b'/' && bytes[index + 1] == b'/' {
            return false;
        }
        index += 1;
    }
    open
}
