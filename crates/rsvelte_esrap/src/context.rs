//! The visitor-facing command builder.
//!
//! A port of esrap's `Context` (`src/context.js`). A [`Context`] accumulates
//! [`Command`]s and tracks whether it has gone multiline, which is the signal
//! visitors use to decide between a one-line and a broken-out layout. Unlike
//! upstream, dispatch (`visit`) lives in the printer (a `match` over oxc node
//! kinds), so `Context` is purely the buffer API: `write`, the whitespace
//! sentinels, `append` (splice a child buffer), `measure`, and `empty`.

use compact_str::CompactString;

use crate::command::Command;

/// Accumulates commands for one syntactic unit. Build a child with
/// [`Context::child`], fill it, then [`Context::append`] it into the parent.
#[derive(Default)]
pub struct Context {
    commands: Vec<Command>,
    has_newline: bool,
    /// Running total of the literal string lengths written so far, so
    /// [`Context::measure`] is O(1) instead of a re-walk of the command tree.
    measure: usize,
    /// `true` once a non-empty literal has been written (the inverse of
    /// [`Context::empty`], tracked for the same reason as `measure`).
    has_content: bool,
    /// `true` once this context (or an appended child) emitted a newline.
    /// Visitors read it to pick a layout. Private with an accessor pair rather
    /// than a public field so that "how many layout decisions did the printer
    /// make" has one place to be counted; as a field there was no way to know
    /// the answer short of enumerating the read sites, which goes quietly stale.
    multiline: bool,
}

impl Context {
    /// A fresh, empty context.
    pub fn new() -> Self {
        let (commands, pool_hit) = crate::pool::take();
        crate::profile::count_context(pool_hit);
        Context {
            commands,
            ..Self::default()
        }
    }

    /// A fresh child context. Named `child` rather than mirroring esrap's `new`
    /// because in this port it carries no shared visitor table.
    pub fn child(&self) -> Context {
        Context::new()
    }

    /// Grow the newline indentation by one level for subsequent newlines.
    pub fn indent(&mut self) {
        crate::profile::count_cmd_layout();
        self.commands.push(Command::Indent);
    }

    /// Shrink the newline indentation by one level.
    pub fn dedent(&mut self) {
        crate::profile::count_cmd_layout();
        self.commands.push(Command::Dedent);
    }

    /// Request a blank line ahead of the next newline.
    pub fn margin(&mut self) {
        crate::profile::count_cmd_layout();
        self.commands.push(Command::Margin);
    }

    /// Emit an indentation-aware newline before the next write. Marks the
    /// context multiline.
    pub fn newline(&mut self) {
        self.has_newline = true;
        crate::profile::count_cmd_layout();
        self.commands.push(Command::Newline);
    }

    /// Emit a single space before the next write.
    pub fn space(&mut self) {
        crate::profile::count_cmd_layout();
        self.commands.push(Command::Space);
    }

    /// Append literal `content`. If a newline is already pending in this
    /// context, writing after it makes the context multiline (mirrors esrap).
    pub fn write(&mut self, content: impl Into<CompactString>) {
        let content = content.into();
        self.measure += content.len();
        self.has_content |= !content.is_empty();
        // `is_heap_allocated` rather than a comparison against the 24-byte
        // inline limit named in `command.rs`'s doc: the limit is the library's
        // to change, and the question is which payloads actually allocated.
        crate::profile::count_cmd_str(content.len(), content.is_heap_allocated());
        self.commands.push(Command::Str(content));
        if self.has_newline {
            self.multiline = true;
        }
    }

    /// Record a source-map anchor (1-based line, 0-based column).
    pub fn location(&mut self, line: u32, column: u32) {
        crate::profile::count_cmd_location();
        self.commands.push(Command::Location { line, column });
    }

    /// `true` once this context (or an appended child) emitted a newline.
    pub fn multiline(&self) -> bool {
        crate::profile::count_multiline_read();
        self.multiline
    }

    /// Force the context multiline, for the cases where a visitor knows a break
    /// is coming that it has not pushed a `Newline` for yet.
    pub fn set_multiline(&mut self) {
        self.multiline = true;
    }

    /// Splice `child`'s commands in place, propagating its multiline state.
    pub fn append(&mut self, child: Context) {
        let child_multiline = child.multiline;
        self.measure += child.measure;
        self.has_content |= child.has_content;
        crate::profile::count_cmd_nested();
        self.commands.push(Command::Nested(child.commands));
        if self.has_newline || child_multiline {
            self.multiline = true;
        }
    }

    /// `true` when nothing with visible content has been written.
    pub fn empty(&self) -> bool {
        crate::profile::count_empty_read();
        !self.has_content
    }

    /// Total length of the literal strings in this context, ignoring whitespace
    /// sentinels — esrap's `measure`, used to decide if a layout fits on a line.
    ///
    /// A `usize` kept current by `write`/`append`, where upstream esrap re-walks
    /// the command array on every call (`context.js`'s recursive `measure`). The
    /// same is true of [`Context::empty`] against upstream's `some(has_content)`.
    /// Worth noting because it means **no layout decision in this port reads the
    /// command tree** — the tree's only consumers are the two flatten drivers.
    pub fn measure(&self) -> usize {
        crate::profile::count_measure_read();
        self.measure
    }

    /// Consume the context, yielding its raw command buffer (for the top-level
    /// [`print`](crate::command::print) call).
    pub fn into_commands(self) -> Vec<Command> {
        self.commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::print;

    #[test]
    fn measure_counts_only_strings() {
        let mut ctx = Context::new();
        ctx.write("abc");
        ctx.space();
        ctx.newline();
        ctx.write("de");
        assert_eq!(ctx.measure(), 5);
    }

    #[test]
    fn empty_ignores_whitespace_sentinels() {
        let mut ctx = Context::new();
        ctx.space();
        ctx.newline();
        ctx.indent();
        assert!(ctx.empty());
        ctx.write("x");
        assert!(!ctx.empty());
    }

    #[test]
    fn append_propagates_multiline() {
        let mut parent = Context::new();
        let mut child = parent.child();
        child.newline();
        child.write("x");
        assert!(child.multiline());
        parent.write("a");
        parent.append(child);
        assert!(parent.multiline());
    }

    #[test]
    fn append_splices_child_output() {
        let mut parent = Context::new();
        parent.write("(");
        let mut child = parent.child();
        child.write("x");
        child.space();
        child.write("y");
        parent.append(child);
        parent.write(")");
        assert_eq!(print(&parent.into_commands(), "\t"), "(x y)");
    }
}
