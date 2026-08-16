//! Drives the real `rsvelte-language-server` binary over stdio and asserts
//! that what it publishes matches calling the formatter / linter directly.

#[path = "support/protocol_server.rs"]
mod protocol_server;

use std::process::Command;

use protocol_server::*;
use serde_json::{Value, json};

#[test]
fn serves_diagnostics_and_formatting() {
    let path = temp_component();
    let uri = file_uri(&path);

    let mut server = Server::start();
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": {
                "workspace": { "configuration": true },
                "textDocument": {
                    "semanticTokens": {
                        "tokenTypes": ["class", "namespace", "operator", "event"],
                        "tokenModifiers": ["local", "declaration", "readonly"]
                    }
                }
            },
        }),
    );
    let result = server.response(id);

    assert_eq!(result["serverInfo"]["name"], "rsvelte-language-server");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    let sync = &result["capabilities"]["textDocumentSync"];
    assert_eq!(sync["openClose"], json!(true));
    // 2 == TextDocumentSyncKind.Incremental
    assert_eq!(sync["change"], json!(2));
    assert_eq!(sync["save"], json!(true));
    assert_eq!(
        result["capabilities"]["documentFormattingProvider"],
        json!(true)
    );
    assert_eq!(
        result["capabilities"]["codeActionProvider"]["codeActionKinds"],
        json!([
            "quickfix",
            "source.organizeImports",
            "source.sortImports",
            "source.removeUnusedImports",
            "source.fixAll",
            "source.fixAll.rsvelte"
        ])
    );
    assert_eq!(result["capabilities"]["positionEncoding"], json!("utf-16"));
    assert_eq!(result["capabilities"]["definitionProvider"], json!(true));
    assert_eq!(
        result["capabilities"]["typeDefinitionProvider"],
        json!(true)
    );
    assert_eq!(
        result["capabilities"]["implementationProvider"],
        json!(true)
    );
    assert_eq!(result["capabilities"]["referencesProvider"], json!(true));
    assert_eq!(
        result["capabilities"]["renameProvider"]["prepareProvider"],
        json!(true)
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"],
        json!(["namespace", "class", "event", "operator"])
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["legend"]["tokenModifiers"],
        json!(["declaration", "readonly", "local"])
    );
    assert_eq!(
        result["capabilities"]["workspace"]["workspaceFolders"],
        json!({ "supported": true, "changeNotifications": true })
    );

    server.notify("initialized", json!({}));
    server.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "svelte",
                "version": 1,
                "text": SOURCE,
            }
        }),
    );

    // Diagnostics must match a direct lint of the same source.
    let mut config_cache = rsvelte_language_server::lint::LintConfigCache::default();
    let config = config_cache.get(path.parent().unwrap());
    let warnings = rsvelte_language_server::settings::CompilerWarnings::default();
    let expected: Vec<Value> = rsvelte_language_server::lint::lint(&path, SOURCE, &config)
        .iter()
        .filter_map(|d| rsvelte_language_server::diagnostics::to_lsp(d, &warnings))
        .map(|d| serde_json::to_value(d).unwrap())
        .collect();
    assert!(!expected.is_empty(), "the fixture should produce findings");
    assert_eq!(server.diagnostics(&uri), expected);

    // Formatting must match a direct in-process format of the same source.
    let session = rsvelte_fmt::FormatSession::resolve(&path).unwrap();
    let formatted = session.format(SOURCE, &path).unwrap();
    assert_ne!(formatted, SOURCE, "the fixture should need reformatting");

    let id = server.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        }),
    );
    let edits = server.response(id);
    let edits = edits.as_array().expect("formatting edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["newText"], json!(formatted));
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );

    // An already-formatted document yields no edits.
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": formatted }],
        }),
    );
    let id = server.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        }),
    );
    assert_eq!(server.response(id), json!([]));

    // Closing clears the document's diagnostics.
    server.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    );
    server.cleared_diagnostics(&uri);

    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn serves_pull_diagnostics_without_push_notifications() {
    let path = temp_component();
    let uri = file_uri(&path);
    let mut server = Server::start();
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "textDocument": { "diagnostic": {} } },
        }),
    );
    let capabilities = server.response(id)["capabilities"].clone();
    assert_eq!(
        capabilities["diagnosticProvider"]["interFileDependencies"],
        json!(false)
    );
    assert_eq!(
        capabilities["diagnosticProvider"]["workspaceDiagnostics"],
        json!(false)
    );
    server.notify("initialized", json!({}));
    did_open(&mut server, &uri, SOURCE);

    let mut config_cache = rsvelte_language_server::lint::LintConfigCache::default();
    let config = config_cache.get(path.parent().unwrap());
    let warnings = rsvelte_language_server::settings::CompilerWarnings::default();
    let expected: Vec<Value> = rsvelte_language_server::lint::lint(&path, SOURCE, &config)
        .iter()
        .filter_map(|d| rsvelte_language_server::diagnostics::to_lsp(d, &warnings))
        .map(|d| serde_json::to_value(d).unwrap())
        .collect();
    let report = server.pull_diagnostics(&uri);
    assert_eq!(report["kind"], json!("full"));
    assert_eq!(report["items"], json!(expected));
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn preprocessing_runs_only_in_trusted_workspaces_and_maps_diagnostics() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let root = temp_dir("preprocess-trusted");
    let marker = root.join("config-executed");
    write_preprocess_fixture(&root, &marker);
    let path = root.join("App.svelte");
    let uri = file_uri(&path);
    let raw = r#"<template lang="pug">p image</template>"#;

    let mut server = workspace_server(&root, true);
    did_open(&mut server, &uri, raw);
    let diagnostics = server.diagnostics_matching(&uri, |diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "a11y_missing_attribute")
    });
    let warning = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == "a11y_missing_attribute")
        .unwrap();
    assert_eq!(
        warning["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert!(marker.is_file());
    let id = server.request("$/getCompiledCode", json!(uri));
    let compiled = server.response(id);
    assert!(compiled["js"]["code"].as_str().unwrap().contains("<img"));
    assert_eq!(compiled["js"]["map"]["version"], json!(3));
    assert!(compiled["css"].is_null());
    assert_eq!(server.shutdown(), Some(0));

    let root = temp_dir("preprocess-untrusted");
    let marker = root.join("config-executed");
    write_preprocess_fixture(&root, &marker);
    let path = root.join("App.svelte");
    let uri = file_uri(&path);
    let mut server = workspace_server(&root, false);
    did_open(&mut server, &uri, raw);
    let _ = server.diagnostics(&uri);
    assert!(!marker.exists(), "untrusted config was executed");
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn compiled_code_matches_the_upstream_wire_shape() {
    let dir = temp_dir("compiled-code");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();
    did_open(
        &mut server,
        &uri,
        "<style>p { color: red; }</style><p>compiled</p>",
    );
    let id = server.request("$/getCompiledCode", json!(uri));
    let result = server.response(id);
    assert!(result["js"]["code"].as_str().unwrap().contains("compiled"));
    assert_eq!(result["js"]["map"]["version"], json!(3));
    let css = result["css"]["code"].as_str().unwrap();
    assert!(css.contains("color") && css.contains("red"));
    assert_eq!(result["css"]["map"]["version"], json!(3));
    assert_eq!(result["css"]["hasGlobal"], json!(false));

    let missing = file_uri(&dir.join("Missing.svelte"));
    let id = server.request("$/getCompiledCode", json!(missing));
    assert!(server.response(id).is_null());
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn extract_component_is_applied_through_workspace_apply_edit() {
    let dir = temp_dir("extract-component");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = Server::start();
    let initialize = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": {
                "workspace": { "configuration": true, "applyEdit": true }
            }
        }),
    );
    let capabilities = server.response(initialize)["capabilities"].clone();
    assert!(
        capabilities["executeCommandProvider"]["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "extract_to_svelte_component")
    );
    assert!(
        capabilities["codeActionProvider"]["codeActionKinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kind| kind == "refactor")
    );
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, "<section>move me</section>");
    let command = server.request(
        "workspace/executeCommand",
        json!({
            "command": "extract_to_svelte_component",
            "arguments": [uri, {
                "uri": uri,
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 26 }
                },
                "filePath": "parts/Moved"
            }]
        }),
    );
    let edit = loop {
        let message = server.read();
        if message["method"] == "workspace/applyEdit" {
            let edit = message["params"]["edit"].clone();
            server.write(&json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": { "applied": true }
            }));
            break edit;
        }
        server.answer_server_request(&message);
    };
    assert_eq!(
        edit["documentChanges"][0]["edits"][0]["newText"],
        "<Moved></Moved>"
    );
    assert!(
        edit["documentChanges"][1]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/parts/Moved.svelte")
    );
    assert!(server.response(command).is_null());
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn official_settings_drive_tag_close_and_strict_attribute_completions() {
    let dir = temp_dir("official-settings");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = Server::start();
    server.official_settings = json!({ "plugin": {
        "html": { "tagComplete": { "enable": false } },
        "svelte": { "format": { "config": { "svelteStrictMode": true } } }
    }});
    let initialize = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } }
        }),
    );
    server.response(initialize);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, "<button on:");
    let close = server.request(
        "html/tag",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 12 }
        }),
    );
    assert!(server.response(close).is_null());
    let click = server
        .completion(&uri, 0, 11)
        .into_iter()
        .find(|item| item["label"] == "on:click")
        .unwrap();
    assert_eq!(click["insertText"], json!("on:click$2=\"{$1}\""));
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn official_diagnostic_switches_gate_svelte_and_css_independently() {
    let dir = temp_dir("official-diagnostic-settings");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = Server::start();
    server.official_settings = json!({ "plugin": {
        "svelte": { "diagnostics": { "enable": false } },
        "css": { "diagnostics": { "enable": true } }
    }});
    let initialize = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } }
        }),
    );
    server.response(initialize);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, "<style>p { colr: red; }</style><img>");
    let css_only = server.diagnostics_matching(&uri, |diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "css_unknown_property")
    });
    assert!(
        !css_only
            .iter()
            .any(|diagnostic| diagnostic["code"] == "a11y_missing_attribute")
    );

    server.official_settings = json!({ "plugin": {
        "svelte": { "diagnostics": { "enable": true } },
        "css": { "diagnostics": { "enable": false } }
    }});
    server.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": Value::Null }),
    );
    server.settle_configuration();
    let svelte_only = server.diagnostics_matching(&uri, |diagnostics| {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "a11y_missing_attribute")
    });
    assert!(
        !svelte_only
            .iter()
            .any(|diagnostic| diagnostic["code"] == "css_unknown_property")
    );
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn inline_initialization_and_configuration_settings_are_merged() {
    let dir = temp_dir("inline-settings");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = Server::start();
    let initialize = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "initializationOptions": { "configuration": { "svelte": { "plugin": {
                "html": { "tagComplete": { "enable": false } },
                "svelte": { "documentHighlight": { "enable": false } }
            }}}},
            "capabilities": {}
        }),
    );
    let capabilities = server.response(initialize)["capabilities"].clone();
    assert_eq!(capabilities["documentHighlightProvider"], json!(false));
    server.notify("initialized", json!({}));
    did_open(&mut server, &uri, "<main>");
    let request_close = |server: &mut Server| {
        let id = server.request(
            "html/tag",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 6 }
            }),
        );
        server.response(id)
    };
    assert!(request_close(&mut server).is_null());
    server.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "svelte": { "plugin": {
            "html": { "tagComplete": { "enable": true } }
        }}}}),
    );
    assert_eq!(request_close(&mut server), json!("</main>"));
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn preprocessing_failure_keeps_raw_language_features_available() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let root = temp_dir("preprocess-failure");
    let marker = root.join("config-executed");
    write_preprocess_fixture(&root, &marker);
    std::fs::write(
        root.join("svelte.config.mjs"),
        r#"
export default {
  preprocess: {
    markup() { throw new Error('fixture preprocessing failure'); }
  }
};
"#,
    )
    .unwrap();
    let path = root.join("App.svelte");
    let uri = file_uri(&path);
    let source = "<style>.x { color: #ffffff; }</style>\n<div>\n  <\n</div>\n";
    let mut server = workspace_server(&root, true);
    did_open(&mut server, &uri, source);
    let diagnostics = server.diagnostics_matching(&uri, |diagnostics| {
        diagnostics.iter().any(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("fixture preprocessing failure"))
        })
    });
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("Preprocessing failed"))
    }));
    assert!(!server.completion(&uri, 2, 3).is_empty());
    assert!(!server.folding_ranges(&uri).is_empty());
    let id = server.request(
        "textDocument/documentColor",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(server.response(id).as_array().map(Vec::len), Some(1));
    assert_eq!(server.shutdown(), Some(0));
}

/// Completion and hover over the wire, on a document that does not parse — the
/// state a component is in for most of the time it is being typed.
#[test]
fn serves_completions_and_hover() {
    let dir = temp_dir("completion");
    let uri = file_uri(&dir.join("App.svelte"));

    let mut server = Server::start();
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    let capabilities = server.response(id)["capabilities"].clone();
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"],
        json!(["<", " ", "#", "@", ":", "/", "|"])
    );
    assert_eq!(capabilities["hoverProvider"], json!(true));
    server.notify("initialized", json!({}));

    let source =
        "<script>\n  let value = 1;\n</script>\n\n<div on:click|>\n  {#each value as v}\n    {#";
    did_open(&mut server, &uri, source);

    // `{#` on the last line, mid-edit: the block completions, closing snippet
    // and all.
    let items = server.completion(&uri, 6, 6);
    assert_eq!(
        items.iter().map(|i| i["label"].clone()).collect::<Vec<_>>(),
        json!(["if", "each", "await :then", "await then", "key", "snippet"])
            .as_array()
            .unwrap()
            .clone()
    );
    let each = items.iter().find(|i| i["label"] == "each").unwrap();
    assert_eq!(each["insertText"], json!("each $1 as $2}\n\t$3\n{/each"));
    // 2 == InsertTextFormat.Snippet, 14 == CompletionItemKind.Keyword
    assert_eq!(each["insertTextFormat"], json!(2));
    assert_eq!(each["kind"], json!(14));
    assert_eq!(each["sortText"], json!("-1"));
    assert_eq!(each["preselect"], json!(true));

    // The open `{#each` decides what a `{/` typed in its place may close.
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{
                "range": {
                    "start": { "line": 6, "character": 5 },
                    "end": { "line": 6, "character": 6 },
                },
                "text": "/",
            }],
        }),
    );
    let items = server.completion(&uri, 6, 6);
    assert_eq!(
        items.iter().map(|i| i["label"].clone()).collect::<Vec<_>>(),
        [json!("each")]
    );

    // Event modifiers, filtered by what the attribute already carries.
    let items = server.completion(&uri, 4, 14);
    let labels: Vec<&str> = items
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            "preventDefault",
            "stopPropagation",
            "passive",
            "nonpassive",
            "capture",
            "once",
            "self",
            "trusted"
        ]
    );
    // 23 == CompletionItemKind.Event
    assert_eq!(items[0]["kind"], json!(23));

    // Hover over `#each` documents the block.
    let hover = server.hover(&uri, 5, 5);
    let contents = hover["contents"]["value"].as_str().unwrap();
    assert!(contents.starts_with("`{#each ...}`"), "{contents}");
    assert_eq!(hover["contents"]["kind"], json!("markdown"));

    // Hover inside `<script>` is the TypeScript plugin's business, not ours.
    assert_eq!(server.hover(&uri, 1, 8), Value::Null);

    // Nothing to offer is `null`, never an error.
    assert_eq!(server.completion(&uri, 1, 0), Vec::<Value>::new());

    assert_eq!(server.shutdown(), Some(0));
}

/// A compiler warning must arrive with the metadata the official server
/// publishes, and the quickfix built from it must come back through the real
/// binary.
#[test]
fn a_compiler_warning_carries_its_docs_link_and_yields_a_quickfix() {
    let dir = temp_dir("code-action");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();

    did_open(&mut server, &uri, "<div>\n    <img>\n</div>\n");
    let diagnostics = server.diagnostics_matching(&uri, |d| {
        d.iter().any(|d| d["code"] == "a11y_missing_attribute")
    });
    let warning = diagnostics
        .iter()
        .find(|d| d["code"] == "a11y_missing_attribute")
        .expect("the fixture should report a missing alt attribute");
    assert_eq!(warning["source"], json!("svelte"));
    assert_eq!(
        warning["codeDescription"]["href"],
        json!("https://svelte.dev/docs/svelte/compiler-warnings#a11y_missing_attribute")
    );

    let id = server.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": warning["range"],
            "context": { "diagnostics": [warning] },
        }),
    );
    let actions = server.response(id);
    let actions = actions.as_array().expect("code actions");
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0]["title"],
        json!("(svelte) Disable a11y_missing_attribute for this line")
    );
    assert_eq!(actions[0]["kind"], json!("quickfix"));
    let edits = actions[0]["edit"]["changes"][&uri]
        .as_array()
        .expect("edits for the document");
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["newText"],
        json!("    <!-- svelte-ignore a11y_missing_attribute -->\n")
    );
    assert_eq!(
        edits[0]["range"],
        json!({ "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } })
    );

    // A request naming no diagnostic, or asking only for another kind, has
    // nothing to fix.
    for context in [
        json!({ "diagnostics": [] }),
        json!({ "diagnostics": [warning], "only": ["refactor"] }),
    ] {
        let id = server.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": warning["range"],
                "context": context,
            }),
        );
        assert_eq!(server.response(id), json!([]));
    }

    assert_eq!(server.shutdown(), Some(0));
}

/// `rsvelte.completion.enable` / `rsvelte.hover.enable` switch the features off
/// without the client having to stop asking.
#[test]
fn the_settings_switch_completion_and_hover_off() {
    let dir = temp_dir("completion-disabled");
    let uri = file_uri(&dir.join("App.svelte"));

    let mut server = Server::start();
    server.settings = json!({
        "completion": { "enable": false },
        "hover": { "enable": false },
    });
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    server.response(id);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, "<p>{#each a as b}{/each}</p>\n{#");

    assert_eq!(server.completion_response(&uri, 1, 2), Value::Null);
    assert_eq!(server.hover(&uri, 0, 5), Value::Null);

    assert_eq!(server.shutdown(), Some(0));
}

/// `compilerWarnings` drops the codes it silences and escalates the rest.
#[test]
fn compiler_warning_settings_reach_the_published_diagnostics() {
    let dir = temp_dir("compiler-warnings");
    let uri = file_uri(&dir.join("App.svelte"));

    let mut server = Server::start();
    server.settings = json!({
        "compilerWarnings": {
            "a11y_missing_attribute": "ignore",
            "a11y_consider_explicit_label": "error",
        }
    });
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    server.response(id);
    server.notify("initialized", json!({}));
    // Opening only once the settings are in hand keeps the first publish from
    // being one a lint that raced them produced.
    server.settle_configuration();
    did_open(&mut server, &uri, "<img>\n<a></a>\n");

    let diagnostics = server.diagnostics(&uri);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d["code"] == "a11y_missing_attribute"),
        "an ignored code must not be published: {diagnostics:?}"
    );
    let escalated = diagnostics
        .iter()
        .find(|d| d["code"] == "a11y_consider_explicit_label")
        .expect("the fixture should report a link without a label");
    // 1 == DiagnosticSeverity.Error
    assert_eq!(escalated["severity"], json!(1));

    assert_eq!(server.shutdown(), Some(0));
}

/// Input that recurses deeper than a default stack allows. The analysis is
/// expected to fail; the session is not.
#[test]
fn a_pathological_document_does_not_take_the_session_down() {
    let dir = temp_dir("pathological");
    let mut server = initialized_server();

    for (name, text) in [
        (
            "nested-elements",
            format!("{}{}", "<div>".repeat(500), "</div>".repeat(500)),
        ),
        (
            "nested-expression",
            format!("<p>{{{}1{}}}</p>", "(".repeat(1000), ")".repeat(1000)),
        ),
        (
            "nested-script",
            format!("<script>{}1{}</script>", "(".repeat(2000), ")".repeat(2000)),
        ),
    ] {
        let uri = file_uri(&dir.join(format!("{name}.svelte")));
        did_open(&mut server, &uri, &text);
        assert!(server.is_alive(), "server died on {name}");
    }

    // And it still serves an ordinary document afterwards.
    let uri = file_uri(&dir.join("Ok.svelte"));
    did_open(&mut server, &uri, "<div>{@html value}</div>\n");
    assert!(
        !server.diagnostics(&uri).is_empty(),
        "diagnostics stopped working"
    );

    assert_eq!(server.shutdown(), Some(0));
}

/// `initialize` payloads whose individual fields this build cannot parse must
/// still be answered — a client left waiting sees an unexplained hang.
#[test]
fn a_malformed_initialize_is_still_answered() {
    for params in [
        json!({ "rootUri": "not a uri", "capabilities": {} }),
        json!({ "capabilities": Value::Null }),
        json!({ "trace": "bogus", "capabilities": {} }),
        json!({}),
    ] {
        let mut server = Server::start();
        let id = server.request("initialize", params.clone());
        let result = server.response(id);
        assert_eq!(
            result["serverInfo"]["name"], "rsvelte-language-server",
            "no initialize result for {params}"
        );
        server.notify("initialized", json!({}));
        assert_eq!(server.shutdown(), Some(0));
    }
}

/// `exit` before `shutdown` is an abnormal end and must be reported as one.
#[test]
fn exit_without_shutdown_fails() {
    let mut server = initialized_server();
    assert_eq!(server.exit(), Some(1));
}

/// A notification arriving between `shutdown` and `exit` is allowed, and must
/// not turn a clean shutdown into a failure.
#[test]
fn a_notification_between_shutdown_and_exit_is_harmless() {
    let mut server = initialized_server();
    let id = server.request("shutdown", Value::Null);
    server.response(id);
    server.notify("$/cancelRequest", json!({ "id": 1 }));
    assert_eq!(server.exit(), Some(0));
}

#[test]
fn cancellation_returns_the_lsp_cancelled_error() {
    let dir = temp_dir("cancel");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();
    did_open(
        &mut server,
        &uri,
        &format!("{}{}", "<div>".repeat(500), "</div>".repeat(500)),
    );
    let id = server.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        }),
    );
    server.notify("$/cancelRequest", json!({ "id": id }));
    let response = server.response_message(id);
    assert_eq!(response["error"]["code"], json!(-32800));
    assert_eq!(server.shutdown(), Some(0));
}

/// Editing `rsvelte-lint.json` reaches the server as a configuration change,
/// and the resolved config must be re-read rather than served from the cache.
#[test]
fn a_config_change_invalidates_the_resolved_lint_config() {
    let dir = temp_dir("lint-config");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();

    fn reports_at_html(diagnostics: &[Value]) -> bool {
        diagnostics
            .iter()
            .any(|d| d["code"] == "svelte/no-at-html-tags")
    }

    did_open(&mut server, &uri, "<div>{@html value}</div>\n");
    server.diagnostics_matching(&uri, reports_at_html);

    std::fs::write(
        dir.join("rsvelte-lint.json"),
        r#"{ "rules": { "svelte/no-at-html-tags": "off" } }"#,
    )
    .unwrap();
    server.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": Value::Null }),
    );

    // The re-lint re-reads the config from disk rather than serving the one it
    // resolved on open, so the rule is gone.
    server.diagnostics_matching(&uri, |d| !reports_at_html(d));

    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn a_watched_config_change_invalidates_the_resolved_lint_config() {
    let dir = temp_dir("watched-lint-config");
    let uri = file_uri(&dir.join("App.svelte"));
    let config_uri = file_uri(&dir.join("rsvelte-lint.json"));
    let mut server = initialized_server();

    fn reports_at_html(diagnostics: &[Value]) -> bool {
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "svelte/no-at-html-tags")
    }

    did_open(&mut server, &uri, "<div>{@html value}</div>\n");
    server.diagnostics_matching(&uri, reports_at_html);
    std::fs::write(
        dir.join("rsvelte-lint.json"),
        r#"{ "rules": { "svelte/no-at-html-tags": "off" } }"#,
    )
    .unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": config_uri, "type": 2 }] }),
    );
    server.diagnostics_matching(&uri, |diagnostics| !reports_at_html(diagnostics));
    assert_eq!(server.shutdown(), Some(0));
}

/// What a VS Code-like client — folding whole lines, reading a symbol tree —
/// gets for a component with elements, blocks, regions, imports and both
/// embedded languages.
#[test]
fn serves_folding_selection_and_symbols() {
    let dir = temp_dir("structure");
    let uri = file_uri(&dir.join("App.svelte"));
    let (mut server, capabilities) = server_with(json!({
        "foldingRange": { "lineFoldingOnly": true },
        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
    }));
    assert_eq!(capabilities["foldingRangeProvider"], json!(true));
    assert_eq!(capabilities["selectionRangeProvider"], json!(true));
    assert_eq!(capabilities["documentSymbolProvider"], json!(true));

    did_open(&mut server, &uri, STRUCTURED);

    let mut folds = server.folding_ranges(&uri);
    folds.sort_by_key(|fold| fold["startLine"].as_u64().unwrap());
    assert_eq!(
        folds,
        json!([
            // The `<script>`, ending on the line before `</script>`.
            { "startLine": 0, "endLine": 4 },
            { "startLine": 1, "endLine": 2, "kind": "imports" },
            { "startLine": 7, "endLine": 13, "kind": "region" },
            { "startLine": 8, "endLine": 11 },
            { "startLine": 9, "endLine": 10 },
            { "startLine": 15, "endLine": 18 },
            // The `<style>` body, folded by indentation.
            { "startLine": 16, "endLine": 17 },
        ])
        .as_array()
        .unwrap()
        .clone(),
        "a line-folding client gets lines only, and one fold per line"
    );

    // The cursor inside `title="row"` on the `<p>`.
    let ranges = server.selection_ranges(&uri, json!([{ "line": 10, "character": 15 }]));
    let ranges = ranges.as_array().expect("one range per position");
    assert_eq!(ranges.len(), 1);
    let mut chain = Vec::new();
    let mut node = &ranges[0];
    loop {
        chain.push(node["range"].clone());
        match node.get("parent") {
            Some(parent) if !parent.is_null() => node = parent,
            _ => break,
        }
    }
    assert_eq!(
        chain,
        vec![
            json!({ "start": { "line": 10, "character": 14 }, "end": { "line": 10, "character": 17 } }),
            json!({ "start": { "line": 10, "character": 7 }, "end": { "line": 10, "character": 18 } }),
            json!({ "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 19 } }),
            json!({ "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 26 } }),
            json!({ "start": { "line": 9, "character": 2 }, "end": { "line": 11, "character": 9 } }),
            json!({ "start": { "line": 8, "character": 0 }, "end": { "line": 12, "character": 6 } }),
        ],
        "value, attribute, start tag, element, each block, div"
    );

    let symbols = server.document_symbols(&uri);
    let names: Vec<Value> = symbols.iter().map(|s| s["name"].clone()).collect();
    assert_eq!(
        names,
        json!(["script", "div.wrap", "style"])
            .as_array()
            .unwrap()
            .clone()
    );
    let each = &symbols[1]["children"][0];
    assert_eq!(each["name"], json!("{#each [1, 2] as n}"));
    // 3 == SymbolKind.Namespace, 8 == SymbolKind.Field
    assert_eq!(each["kind"], json!(3));
    assert_eq!(each["children"][0]["name"], json!("p"));
    assert_eq!(each["children"][0]["kind"], json!(8));
    assert!(
        symbols[0]["location"].is_null(),
        "a tree carries ranges, not locations"
    );

    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn serves_runes_legacy_mode_code_lenses() {
    let dir = temp_dir("code-lens");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();

    did_open(&mut server, &uri, "<script>let count = $state(0);</script>");
    assert_eq!(
        server.code_lenses(&uri),
        json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 0 }
            },
            "command": {
                "title": "Runes mode",
                "command": "svelte.openLink",
                "arguments": ["https://svelte.dev/docs/svelte/legacy-overview"]
            }
        }])
        .as_array()
        .unwrap()
        .clone()
    );

    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": "<script>let count = 0;</script>" }],
        }),
    );
    assert_eq!(
        server.code_lenses(&uri)[0]["command"]["title"],
        "Legacy mode"
    );
    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn code_lens_respects_its_setting() {
    let dir = temp_dir("code-lens-disabled");
    let uri = file_uri(&dir.join("App.svelte"));
    let mut server = initialized_server();
    server.settings = json!({ "runesLegacyModeCodeLens": { "enable": false } });
    server.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": Value::Null }),
    );
    server.settle_configuration();
    did_open(&mut server, &uri, "<p>legacy</p>");
    assert!(server.code_lenses(&uri).is_empty());
    assert_eq!(server.shutdown(), Some(0));
}

/// A client that declares neither capability gets folding ranges with
/// characters and a flat `SymbolInformation` list.
#[test]
fn a_client_without_the_modern_capabilities_is_served_the_old_shapes() {
    let dir = temp_dir("structure-flat");
    let path = dir.join("App.svelte");
    let uri = file_uri(&path);
    let (mut server, _) = server_with(json!({}));
    did_open(&mut server, &uri, STRUCTURED);

    let folds = server.folding_ranges(&uri);
    let script = folds
        .iter()
        .find(|fold| fold["startLine"] == json!(0))
        .expect("the script folds");
    assert_eq!(
        *script,
        json!({
            "startLine": 0,
            "startCharacter": 0,
            "endLine": 5,
            "endCharacter": 9,
        }),
        "without lineFoldingOnly the whole span is reported"
    );

    let symbols = server.document_symbols(&uri);
    let flat: Vec<(Value, Value)> = symbols
        .iter()
        .map(|s| (s["name"].clone(), s["containerName"].clone()))
        .collect();
    assert_eq!(
        flat,
        vec![
            (json!("script"), Value::Null),
            (json!("div.wrap"), Value::Null),
            (json!("{#each [1, 2] as n}"), json!("div.wrap")),
            (json!("p"), json!("{#each [1, 2] as n}")),
            (json!("style"), Value::Null),
        ]
    );
    assert_eq!(symbols[0]["location"]["uri"], json!(uri));
    assert!(
        symbols[0]["children"].is_null(),
        "a flat list has no children"
    );

    assert_eq!(server.shutdown(), Some(0));
}

/// The three `rsvelte.*` switches, and the shapes a switched-off provider still
/// has to answer with.
#[test]
fn the_settings_switch_the_structure_providers_off() {
    let dir = temp_dir("structure-disabled");
    let uri = file_uri(&dir.join("App.svelte"));

    let mut server = Server::start();
    server.settings = json!({
        "foldingRange": { "enable": false },
        "selectionRange": { "enable": false },
        "documentSymbol": { "enable": false },
    });
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    server.response(id);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    did_open(&mut server, &uri, STRUCTURED);

    assert_eq!(server.folding_ranges(&uri), Vec::<Value>::new());
    assert_eq!(
        server.selection_ranges(&uri, json!([{ "line": 10, "character": 15 }])),
        Value::Null
    );
    assert_eq!(server.document_symbols(&uri), Vec::<Value>::new());

    assert_eq!(server.shutdown(), Some(0));
}

/// A half-written or pathological document must cost at most an empty answer.
#[test]
fn the_structure_providers_survive_documents_that_do_not_parse() {
    let dir = temp_dir("structure-broken");
    let mut server = initialized_server();

    for (name, text) in [
        ("stray-close", "<div>x</div>\n</span>".to_string()),
        ("half-block", "<p>💡</p>\n{#each items as ".to_string()),
        (
            "half-script",
            "<script>\n  const a = {\n</script>".to_string(),
        ),
        (
            "deep",
            format!("{}{}", "<div>\n".repeat(300), "</div>\n".repeat(300)),
        ),
    ] {
        let uri = file_uri(&dir.join(format!("{name}.svelte")));
        did_open(&mut server, &uri, &text);
        // Every one of these must come back, whatever it comes back with.
        server.folding_ranges(&uri);
        server.document_symbols(&uri);
        server.selection_ranges(&uri, json!([{ "line": 1, "character": 1 }]));
        assert!(server.is_alive(), "server died on {name}");
    }

    // An unknown document is answered too, rather than left pending.
    let missing = file_uri(&dir.join("Missing.svelte"));
    assert_eq!(server.folding_ranges(&missing), Vec::<Value>::new());
    assert_eq!(server.document_symbols(&missing), Vec::<Value>::new());
    assert_eq!(
        server.selection_ranges(&missing, json!([{ "line": 0, "character": 0 }])),
        Value::Null
    );

    assert_eq!(server.shutdown(), Some(0));
}

#[test]
fn file_references_run_on_the_worker_and_are_sorted() {
    let dir = temp_dir("file-references");
    let target = file_uri(&dir.join("Target.svelte"));
    let first = file_uri(&dir.join("A.svelte"));
    let second = file_uri(&dir.join("B.svelte"));
    let mut server = initialized_server();
    did_open(&mut server, &target, "<p>target</p>");
    did_open(
        &mut server,
        &second,
        "<script>import Target from './Target.svelte';</script>",
    );
    did_open(
        &mut server,
        &first,
        "<script>import './Target.svelte';</script>",
    );

    let id = server.request("$/getFileReferences", json!(target));
    let locations = server.response(id);
    assert_eq!(locations.as_array().map(Vec::len), Some(2));
    assert_eq!(locations[0]["uri"], json!(first));
    assert_eq!(locations[1]["uri"], json!(second));

    assert_eq!(server.shutdown(), Some(0));
}
