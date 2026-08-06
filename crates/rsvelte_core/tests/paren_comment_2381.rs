//! A source paren carrying an interior comment must not survive into the output.
//!
//! Svelte parses `<script>` with acorn and *without* `preserveParens`
//! (`phases/1-parse/acorn.js:50-55`; `preserveParens: true` appears only in
//! `parse_expression_at`, `:90`), so official's script AST holds no
//! `ParenthesizedExpression` at all and every printed paren is recomputed from
//! precedence. rsvelte parses with `preserve_parens: true` and unwraps in the
//! printer instead, which is equivalent only if the unwrap is unconditional.
//!
//! Every expected string below is the official compiler's output for the same
//! source (svelte 5.56.8), not rsvelte's.

use rsvelte_core::{CompileOptions, GenerateMode, compile, compiler::CssMode};

fn client(src: &str) -> String {
    compile(
        src,
        CompileOptions {
            filename: Some("T.svelte".to_string()),
            generate: GenerateMode::Client,
            dev: false,
            css: CssMode::External,
            ..Default::default()
        },
    )
    .expect("compile")
    .js
    .code
}

fn module(body: &str) -> String {
    client(&format!("<script module>\n{body}\n</script>\n<p>hi</p>\n"))
}

/// Killed by restoring the `comment_in_span` branch that kept the literal
/// parens: that branch emitted `((await /* hi */ load()))()`, doubling the pair
/// the callee rule already adds for an `AwaitExpression` in callee position.
#[test]
fn a_comment_does_not_preserve_parens_around_an_await_callee() {
    let out = module("\texport async function f() {\n\t\treturn (await /* hi */ load())();\n\t}");
    assert!(
        out.contains("\treturn (await /* hi */ load())();\n"),
        "{out}"
    );
    assert!(!out.contains("((await"), "{out}");
}

/// Same defect reached through the instance script rather than `<script module>`,
/// and with the comment inside the call arguments rather than before the callee.
/// Killed by the same mutant.
#[test]
fn the_comment_may_sit_anywhere_inside_the_paren_span() {
    let out = module("\texport async function f() {\n\t\treturn (await load(/* hi */ 1))();\n\t}");
    assert!(
        out.contains("\treturn (await load(/* hi */ 1))();\n"),
        "{out}"
    );

    let instance = client(
        "<script>\n\tasync function f() {\n\t\treturn (await /* hi */ load())();\n\t}\n</script>\n<p>{f}</p>\n",
    );
    assert!(
        instance.contains("\t\treturn (await /* hi */ load())();\n"),
        "{instance}"
    );
}

/// The defect was never specific to `await`: the kept parens doubled or appeared
/// spuriously in every position. Each assertion is killed by the same mutant.
#[test]
fn a_comment_does_not_preserve_parens_in_any_position() {
    let cases: &[(&str, &str)] = &[
        // redundant parens the grammar does not need: official drops them and
        // re-homes the comment as leading trivia of the inner expression
        (
            "\texport function g(x) {\n\t\tconst y = (/* c */ x);\n\t\treturn y;\n\t}",
            "\tconst y = /* c */ x;\n",
        ),
        (
            "\texport function g(x) {\n\t\treturn foo((/* c */ x));\n\t}",
            "\treturn foo(/* c */ x);\n",
        ),
        (
            "\texport function g(x) {\n\t\tif ((/* c */ x)) return 1;\n\t\treturn 0;\n\t}",
            "\tif (/* c */ x) return 1;\n",
        ),
        (
            "\texport function g() {\n\t\treturn new (/* c */ Foo)();\n\t}",
            "\treturn new /* c */ Foo();\n",
        ),
        (
            "\texport function g(a) {\n\t\tconst y = (/* c */ a) + 1;\n\t\treturn y;\n\t}",
            "\tconst y = /* c */ a + 1;\n",
        ),
        (
            "\texport function g(a, b) {\n\t\tconst y = ((/* c */ a, b));\n\t\treturn y;\n\t}",
            "\tconst y = /* c */ (a, b);\n",
        ),
        // parens the grammar DOES need: exactly one pair, comment inside it
        (
            "\texport function g(a, b) {\n\t\tconst y = (/* c */ a || b) && 1;\n\t\treturn y;\n\t}",
            "\tconst y = (/* c */ a || b) && 1;\n",
        ),
        (
            "\texport async function f(a, b) {\n\t\tconst y = await (/* c */ a || b);\n\t\treturn y;\n\t}",
            "\tconst y = await (/* c */ a || b);\n",
        ),
    ];
    for (body, expected) in cases {
        let out = module(body);
        assert!(out.contains(expected), "expected {expected:?} in\n{out}");
    }
}

/// A line comment is the ASI-hazard case the deleted branch claimed to protect.
/// Official drops the parens outside a `return` here too, so the branch was not
/// protecting anything the `ReturnStatement` rule does not already cover.
/// Killed by the same mutant.
#[test]
fn a_line_comment_does_not_preserve_parens_either() {
    let out = module("\texport function g(x) {\n\t\tconst y = (// hey\n\t\t\tx);\n\t\treturn y;\n\t}");
    assert!(out.contains("\tconst y = // hey\n\tx;\n"), "{out}");

    // inside a `return`, the parens survive — but via esrap's own rule
    let ret = module("\texport function g(x) {\n\t\treturn (// hey\n\t\t\tx);\n\t}");
    assert!(ret.contains("\treturn (// hey\n\tx);\n"), "{ret}");
}

/// GUARD, not coverage: passes before and after the fix. Inert against the
/// shipped change because `return (/* c */ x);` is produced by two different
/// mechanisms either side of it — the deleted `comment_in_span` branch before,
/// and esrap's own `ReturnStatement` comment wrap
/// (`esrap/src/languages/ts/index.js:1516-1530`) after.
///
/// It is *not* inert against the partial mutant "delete `comment_in_span` but
/// leave `ReturnStatement` comparing `arg.span().start`": with a paren node the
/// argument span starts at `(`, before the comment, so the wrap never fires and
/// the output degrades to `return /* c */ x;`. That is what the `unparen(arg)`
/// call in the `ReturnStatement` arm exists for.
#[test]
fn guard_return_still_brackets_a_comment_before_its_argument() {
    let out = module("\texport function g(x) {\n\t\treturn (/* c */ x);\n\t}");
    assert!(out.contains("\treturn (/* c */ x);\n"), "{out}");
}

/// GUARD, not coverage: passes before and after. Inert because the defect only
/// ever fired when a comment sat inside the paren span — with no comment the
/// printer already took the unconditional-unwrap path on both sides.
///
/// It is not inert against the over-correction "make
/// `ParenthesizedExpression` always keep its parens", which would print
/// `return (x);` and `return ((await load()))();`.
#[test]
fn guard_parens_without_a_comment_are_unchanged() {
    let dropped = module("\texport function g(x) {\n\t\treturn (x);\n\t}");
    assert!(dropped.contains("\treturn x;\n"), "{dropped}");

    let kept = module("\texport async function f() {\n\t\treturn (await load())();\n\t}");
    assert!(kept.contains("\treturn (await load())();\n"), "{kept}");
}
