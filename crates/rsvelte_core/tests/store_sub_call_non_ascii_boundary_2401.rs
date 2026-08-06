//! A store-subscription name that is only a *suffix* of a longer identifier
//! must not be treated as a standalone store read.
//!
//! The client store-call pre-pass classified the preceding character with
//! `result.as_bytes()[pos - 1] as char`, a Latin-1 decode: for `名$count` the
//! byte before `$count` is `0x8D`, the last continuation byte of `名`, which
//! decodes to U+008D — a control character, so the "not an identifier
//! character" test passed and the local `名$count` was rewritten to
//! `名$count()`.

use rsvelte_core::{CompileOptions, GenerateMode, compile};

fn compile_client(src: &str) -> String {
    compile(
        src,
        CompileOptions {
            filename: Some("App.svelte".to_string()),
            generate: GenerateMode::Client,
            ..Default::default()
        },
    )
    .expect("compile")
    .js
    .code
}

/// The mixed shape: a standalone `$count(…)` on the same line keeps the
/// identifier pre-filter open, so the boundary test still runs on `名$count`.
const MIXED: &str = r#"<script>
	import { writable } from 'svelte/store';
	const count = writable(0);
	const 名$count = (v) => v;
	let x = $count(1) + 名$count(1);
</script>

<p>{x} {$count}</p>
"#;

const ASCII_MIXED: &str = r#"<script>
	import { writable } from 'svelte/store';
	const count = writable(0);
	const a$count = (v) => v;
	let x = $count(1) + a$count(1);
</script>

<p>{x} {$count}</p>
"#;

/// Discriminating on current `main`. The standalone `$count(1)` holds the
/// pre-filter open — the case #2389 does not close — so before the fix the
/// emitted module contained `名$count()(1)`, calling a local as a getter.
#[test]
fn non_ascii_prefixed_local_is_not_a_store_read() {
    let out = compile_client(MIXED);
    assert!(
        !out.contains("名$count()"),
        "local call was rewritten as a store read:\n{out}"
    );
}

/// Guard, not discriminating: an ASCII byte and its `char` are the same value,
/// so the cast is a no-op here and this passes before and after. It is the
/// control that isolates the failure to the decode rather than to suffix
/// matching in general — after the fix both rows agree.
#[test]
fn ascii_prefixed_local_is_not_a_store_read() {
    let out = compile_client(ASCII_MIXED);
    assert!(
        !out.contains("a$count()"),
        "local call was rewritten as a store read:\n{out}"
    );
}
