---
"@rsvelte/compiler": patch
---

fix(compiler): unwrap a source paren even when it holds a comment

Svelte parses `<script>` with acorn and without `preserveParens`, so official's
script AST holds no `ParenthesizedExpression` and every printed paren comes from
precedence. rsvelte parses with `preserve_parens: true` and compensates by
unwrapping in the printer, but kept the literal parens whenever a comment sat
inside the span. That doubled any pair the grammar also required — `return
((await /* hi */ load()))();` for official's `return (await /* hi */ load())();`
— and added a spurious pair everywhere else, including declarator initialisers,
call arguments, `if` tests, `new` callees and sequences.

The unwrap is now unconditional. `return (/* c */ x);`, the one shape whose
parens are real, keeps them through esrap's own `ReturnStatement` rule, which now
measures the comment against the unparenthesized argument as upstream does.
