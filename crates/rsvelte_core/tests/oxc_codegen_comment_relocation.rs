use rsvelte_core::{CompileOptions, GenerateMode, compile};

fn client(source: &str) -> String {
    compile(
        source,
        CompileOptions {
            generate: GenerateMode::Client,
            filename: Some("comments.svelte".into()),
            ..Default::default()
        },
    )
    .expect("compile")
    .js
    .code
}

fn assert_parses(code: &str) {
    let allocator = oxc_allocator::Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, code, oxc_span::SourceType::mjs()).parse();
    assert!(
        parsed.diagnostics.is_empty(),
        "{:?}\n{code}",
        parsed.diagnostics
    );
}

#[test]
fn inline_parameter_comment_stays_at_the_parameter() {
    let code = client(
        "<script>\nlet xs = $state([]);\nlet path = $derived(xs.map((/** @type {object} */ d) => d));\n/** @type {string} */\nlet area = $derived(path);\n</script>",
    );
    assert!(
        code.contains(".map((/** @type {object} */ d) => d)"),
        "{code}"
    );
    assert!(code.contains("/** @type {string} */\n\tlet area"), "{code}");
    assert_parses(&code);
}

#[test]
fn chain_comments_stay_before_the_following_member() {
    let code = client(
        "<script>\nlet chain;\nlet value = $derived(chain.a(1)\n// @ts-expect-error\n.b(2)\n// @ts-expect-error\n.c(3));\n</script>",
    );
    assert!(code.contains("// @ts-expect-error\n.b(2)"), "{code}");
    assert!(code.contains("// @ts-expect-error\n.c(3)"), "{code}");
    assert_parses(&code);
}

#[test]
fn non_ascii_before_a_relocated_comment_does_not_split_utf8() {
    let code = client(
        "<script>\nlet label = 'é';\nlet value = label\n// explain\n.replace('é', 'e');\n</script>",
    );
    assert!(code.contains("// explain\n.replace"), "{code}");
    assert_parses(&code);
}

#[test]
fn trailing_line_comment_stays_before_generated_declarations() {
    let code = client("<script>\n// c\n</script>\n\n<button>x</button>");
    assert!(code.contains("var // c\n button = root();"), "{code}");
    assert_parses(&code);
}
