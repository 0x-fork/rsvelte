//! Robustness of the real `rsvelte-language-server` binary: what it does with
//! input no well-behaved editor sends.
//!
//! Every case here asserts the same thing in the end — the server is still
//! answering. A language server that dies takes every feature with it, and the
//! parity gate (`scripts/compat-corpus/lsp-verify.mjs`) cannot see this class at
//! all: it drives both servers with well-formed traffic, so a crash on a
//! malformed frame would show up there as an empty response set, if at all.

#[path = "support/protocol_server.rs"]
mod protocol_server;

use protocol_server::*;
use serde_json::{Value, json};

/// A frame whose body is not JSON, one that is JSON but not a JSON-RPC message,
/// and requests whose params have the wrong shape. None may take the server down.
#[test]
fn survives_malformed_messages() {
    let mut server = initialized_server();

    for body in [
        "{ this is not json",
        "[]",
        "null",
        r#"{"jsonrpc":"2.0"}"#,
        r#"{"jsonrpc":"2.0","method":42}"#,
    ] {
        server.write_raw(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    }

    // Well-formed frames carrying params of the wrong type. A request must draw
    // *some* response — silence would leave a real client waiting forever.
    for (method, params) in [
        ("textDocument/hover", json!(42)),
        (
            "textDocument/hover",
            json!({ "textDocument": { "uri": 7 } }),
        ),
        ("textDocument/formatting", json!({})),
        ("textDocument/completion", json!({ "position": "middle" })),
    ] {
        let id = server.request(method, params);
        let message = server.response_message(id);
        assert!(
            message.get("result").is_some() || message.get("error").is_some(),
            "{method} with malformed params drew neither a result nor an error"
        );
    }

    // Notifications the server cannot act on: an unknown one, a change to a
    // document that was never opened, and a close of the same.
    server.notify("rsvelte/nonsense", json!({ "anything": true }));
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": "file:///never-opened.svelte", "version": 2 },
            "contentChanges": [{ "text": "<p>hi</p>" }],
        }),
    );
    server.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": "file:///never-opened.svelte" } }),
    );

    assert!(server.is_alive(), "server died on malformed input");
}

/// The document is unparseable at nearly every intermediate keystroke. The
/// server has to keep answering through all of them — this is the ordinary
/// state of a file being typed into, not an edge case.
#[test]
fn answers_through_mid_edit_parse_errors() {
    let path = temp_dir("mid-edit").join("App.svelte");
    let uri = file_uri(&path);
    let mut server = initialized_server();
    did_open(&mut server, &uri, "<div>done</div>\n");

    let broken = [
        "<div",
        "<div class=",
        "<div class=\"a",
        "{#if",
        "{#if cond}",
        "{#each items as",
        "<script>let value = </script>",
        "<script>let value = 'unterminated</script>",
        "<script>function f( {</script>",
        "<style>.a { color:</style>",
        "{@html",
        "<div>{#snippet",
    ];

    for (version, text) in broken.iter().enumerate() {
        server.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version + 2 },
                "contentChanges": [{ "text": text }],
            }),
        );
        // One request per provider family, so a panic in any of them surfaces
        // against the edit that caused it rather than at the end of the run.
        for (method, params) in [
            (
                "textDocument/hover",
                json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 3 } }),
            ),
            (
                "textDocument/foldingRange",
                json!({ "textDocument": { "uri": uri } }),
            ),
            (
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
            ),
            (
                "textDocument/completion",
                json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 3 } }),
            ),
        ] {
            let id = server.request(method, params);
            let message = server.response_message(id);
            assert!(
                message.get("result").is_some() || message.get("error").is_some(),
                "{method} answered nothing while the document was `{text}`"
            );
        }
    }

    // And it recovers: a valid document after all that still produces symbols.
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 100 },
            "contentChanges": [{ "text": STRUCTURED }],
        }),
    );
    assert!(
        !server.document_symbols(&uri).is_empty(),
        "no symbols after recovering from the broken edits"
    );
}

/// A tsgo child that dies the moment it is spawned. TypeScript features are
/// unavailable, but the native ones must keep working and the server must not
/// follow the child down — nor spin restarting it.
#[cfg(unix)]
#[test]
fn survives_a_tsgo_child_that_cannot_start() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_dir("tsgo-crash");
    let fake = root.join("fake-tsgo");
    std::fs::write(&fake, "#!/bin/sh\nexit 7\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = root.join("App.svelte");
    std::fs::write(&path, SOURCE).unwrap();
    let uri = file_uri(&path);

    let mut server = Server::start_with_env(&[("TSGO_BIN", &fake.display().to_string())]);
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": file_uri(&root),
            "initializationOptions": { "isTrusted": false },
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    server.response(id);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, STRUCTURED);

    // Native providers, which never reach the child.
    assert!(!server.folding_ranges(&uri).is_empty());
    assert!(!server.document_symbols(&uri).is_empty());

    // A request that IS routed to the child must still come back, one way or
    // the other, instead of hanging on a process that is not there.
    let id = server.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 4, "character": 7 } }),
    );
    let message = server.response_message(id);
    assert!(message.get("result").is_some() || message.get("error").is_some());

    assert!(server.is_alive(), "server died with its tsgo child");
}

/// Two hundred requests, every one cancelled immediately. The protocol allows a
/// server to answer a cancelled request normally or with `RequestCancelled`, but
/// not to drop it: a client that never hears back leaks the id forever.
#[test]
fn survives_a_cancellation_storm() {
    let path = temp_dir("cancel-storm").join("App.svelte");
    let uri = file_uri(&path);
    let mut server = initialized_server();
    did_open(&mut server, &uri, STRUCTURED);

    let mut ids = Vec::new();
    for _ in 0..200 {
        let id = server.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        );
        server.notify("$/cancelRequest", json!({ "id": id }));
        ids.push(id);
    }

    let mut answered = std::collections::HashSet::new();
    while answered.len() < ids.len() {
        let message = server.read();
        if let Some(id) = message.get("id").and_then(Value::as_i64)
            && message.get("method").is_none()
        {
            answered.insert(id);
            continue;
        }
        server.answer_server_request(&message);
    }

    assert_eq!(
        answered.len(),
        ids.len(),
        "a cancelled request was never answered"
    );
    assert!(server.is_alive(), "server died under a cancellation storm");
}
