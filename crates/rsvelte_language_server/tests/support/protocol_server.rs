//! The stdio harness the language-server integration tests drive the real
//! `rsvelte-language-server` binary through: JSON-RPC framing, id
//! correlation, answering the requests the server makes of its client, and a
//! watchdog so a protocol bug fails instead of hanging the run.
//!
//! Shared by `protocol.rs` and `robustness.rs`; each uses a subset.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

/// A component with an unformatted tag and a lint finding.
pub const SOURCE: &str = "<div   class='a'>{@html value}</div>\n";

pub struct Server {
    pub child: Arc<Mutex<Child>>,
    /// Taken (and thereby closed) on shutdown, so the server sees EOF.
    pub stdin: Option<ChildStdin>,
    pub stdout: BufReader<ChildStdout>,
    pub finished: Arc<AtomicBool>,
    pub next_id: i64,
    /// What `workspace/configuration` is answered with.
    pub settings: Value,
    pub official_settings: Value,
}

impl Server {
    pub fn start() -> Self {
        Self::start_with_env(&[])
    }

    pub fn start_with_env(vars: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rsvelte-language-server"));
        for (key, value) in vars {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn language server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        // A protocol bug would otherwise hang the test run forever.
        let child = Arc::new(Mutex::new(child));
        let finished = Arc::new(AtomicBool::new(false));
        {
            let child = Arc::clone(&child);
            let finished = Arc::clone(&finished);
            std::thread::spawn(move || {
                for _ in 0..600 {
                    if finished.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.lock().unwrap().kill();
            });
        }

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            finished,
            next_id: 0,
            settings: json!({
                "format": { "enable": true },
                "lint": { "enable": true },
                "completion": { "enable": true },
                "hover": { "enable": true },
            }),
            official_settings: Value::Null,
        }
    }

    /// Write errors are swallowed: a server that died shows up far more
    /// clearly as a failed read than as a broken pipe here.
    pub fn write(&mut self, message: &Value) {
        let body = serde_json::to_string(message).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        let _ = write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = stdin.flush();
    }

    /// Bytes straight onto the wire, framing and all — the only way to send a
    /// message the framing layer itself has to reject.
    pub fn write_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        let _ = stdin.write_all(bytes);
        let _ = stdin.flush();
    }

    pub fn read(&mut self) -> Value {
        self.try_read().expect("server closed the connection")
    }

    /// `None` once the server's stdout reaches EOF, i.e. it exited.
    pub fn try_read(&mut self) -> Option<Value> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0u8; length.expect("Content-Length header")];
        self.stdout.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
    }

    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Read until the response to `id` arrives, answering any
    /// `workspace/configuration` the server asks for on the way.
    pub fn response(&mut self, id: i64) -> Value {
        self.response_message(id)["result"].clone()
    }

    pub fn response_message(&mut self, id: i64) -> Value {
        loop {
            let message = self.read();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return message;
            }
            self.answer_server_request(&message);
        }
    }

    /// Answer the `workspace/configuration` the server asks for once it has
    /// seen `initialized`, so that a request sent afterwards is served with
    /// this client's settings rather than the defaults.
    pub fn settle_configuration(&mut self) {
        loop {
            let message = self.read();
            let configuration = message["method"] == "workspace/configuration";
            self.answer_server_request(&message);
            if configuration {
                return;
            }
        }
    }

    /// The items `textDocument/completion` offers at a position.
    pub fn completion(&mut self, uri: &str, line: u32, character: u32) -> Vec<Value> {
        let response = self.completion_response(uri, line, character);
        response["items"].as_array().cloned().unwrap_or_default()
    }

    pub fn completion_response(&mut self, uri: &str, line: u32, character: u32) -> Value {
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        );
        self.response(id)
    }

    pub fn folding_ranges(&mut self, uri: &str) -> Vec<Value> {
        let id = self.request(
            "textDocument/foldingRange",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.response(id).as_array().cloned().unwrap_or_default()
    }

    pub fn selection_ranges(&mut self, uri: &str, positions: Value) -> Value {
        let id = self.request(
            "textDocument/selectionRange",
            json!({ "textDocument": { "uri": uri }, "positions": positions }),
        );
        self.response(id)
    }

    pub fn document_symbols(&mut self, uri: &str) -> Vec<Value> {
        let id = self.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.response(id).as_array().cloned().unwrap_or_default()
    }

    pub fn code_lenses(&mut self, uri: &str) -> Vec<Value> {
        let id = self.request(
            "textDocument/codeLens",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.response(id).as_array().cloned().unwrap_or_default()
    }

    pub fn hover(&mut self, uri: &str, line: u32, character: u32) -> Value {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        );
        self.response(id)
    }

    pub fn pull_diagnostics(&mut self, uri: &str) -> Value {
        let id = self.request(
            "textDocument/diagnostic",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.response(id)
    }

    /// Read until diagnostics for `uri` arrive.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.read();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
            self.answer_server_request(&message);
        }
    }

    /// Read publishes for `uri` until one satisfies `want`, so a publish still
    /// in flight from an earlier change cannot decide an assertion.
    pub fn diagnostics_matching(
        &mut self,
        uri: &str,
        want: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        for _ in 0..8 {
            let diagnostics = self.diagnostics(uri);
            if want(&diagnostics) {
                return diagnostics;
            }
        }
        panic!("diagnostics for {uri} never reached the expected state");
    }

    /// Read until `uri`'s diagnostics are cleared, skipping any publish a
    /// debounced lint got in first.
    pub fn cleared_diagnostics(&mut self, uri: &str) {
        self.diagnostics_matching(uri, <[Value]>::is_empty);
    }

    pub fn answer_server_request(&mut self, message: &Value) {
        let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id")) else {
            return;
        };
        let result = if method == "workspace/configuration" {
            let values = message["params"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| match item["section"].as_str() {
                    Some("rsvelte") => self.settings.clone(),
                    Some("svelte") => self.official_settings.clone(),
                    _ => Value::Null,
                })
                .collect();
            Value::Array(values)
        } else {
            Value::Null
        };
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    pub fn shutdown(&mut self) -> Option<i32> {
        let id = self.request("shutdown", Value::Null);
        self.response(id);
        self.exit()
    }

    /// Send `exit` and wait for the process, returning its exit code.
    pub fn exit(&mut self) -> Option<i32> {
        self.notify("exit", Value::Null);
        // Closing the pipe is what a real client's process exit does, and it is
        // what ends the server's reader thread — without it the server never
        // finishes shutting down.
        self.stdin.take();
        // Polled rather than `wait()`ed so the watchdog can still take the lock
        // and kill a server that refuses to exit.
        let mut code = None;
        for _ in 0..300 {
            if let Ok(Some(status)) = self.child.lock().unwrap().try_wait() {
                code = Some(status.code().unwrap_or(-1));
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.finished.store(true, Ordering::Relaxed);
        code
    }

    /// Whether the server still answers. An unknown method must always draw a
    /// `MethodNotFound` response, which makes it a cheap liveness probe.
    pub fn is_alive(&mut self) -> bool {
        if matches!(self.child.lock().unwrap().try_wait(), Ok(Some(_))) {
            return false;
        }
        let id = self.request("rsvelte/ping", Value::Null);
        loop {
            let Some(message) = self.try_read() else {
                return false;
            };
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return true;
            }
            self.answer_server_request(&message);
        }
    }
}

impl Drop for Server {
    /// A surviving child keeps cargo's inherited stderr pipe open, which wedges
    /// the whole test run — so it is killed even when the test panicked.
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.lock().unwrap().kill();
    }
}

/// A directory of this test's own, so a config file one case writes cannot
/// reach another's documents.
pub fn temp_dir(case: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rsvelte-ls-protocol-{}-{case}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn temp_component() -> PathBuf {
    temp_dir("format").join("App.svelte")
}

pub fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// `initialize` + `initialized`, declaring `workspace/configuration` support.
pub fn initialized_server() -> Server {
    let mut server = Server::start();
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
    server
}

pub fn workspace_server(root: &Path, is_trusted: bool) -> Server {
    let mut server = Server::start();
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": file_uri(root),
            "initializationOptions": { "isTrusted": is_trusted },
            "capabilities": { "workspace": { "configuration": true } },
        }),
    );
    server.response(id);
    server.notify("initialized", json!({}));
    server.settle_configuration();
    server
}

pub fn write_preprocess_fixture(root: &Path, marker: &Path) {
    let package = root.join("node_modules/svelte");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"svelte","type":"module","exports":{"./compiler":"./compiler.js"}}"#,
    )
    .unwrap();
    std::fs::write(
        package.join("compiler.js"),
        r#"
export async function preprocess(source, configured, options) {
  const group = Array.isArray(configured) ? configured[0] : configured;
  const result = await group.markup({ content: source, filename: options?.filename });
  return { code: result.code, map: result.map, dependencies: result.dependencies ?? [] };
}
"#,
    )
    .unwrap();
    let marker = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    std::fs::write(
        root.join("svelte.config.mjs"),
        format!(
            r#"
import {{ writeFileSync }} from 'node:fs';
writeFileSync({marker}, 'executed');
export default {{
  preprocess: {{
    markup({{ content }}) {{
      return {{
        code: '<img>',
        map: {{
          version: 3,
          sources: ['App.svelte'],
          names: [],
          mappings: 'AAAA',
          sourcesContent: [content]
        }}
      }};
    }}
  }}
}};
"#
        ),
    )
    .unwrap();
}

/// The same, with `textDocument` capabilities of the client's choosing, and the
/// capabilities the server answered with.
pub fn server_with(text_document: Value) -> (Server, Value) {
    let mut server = Server::start();
    let id = server.request(
        "initialize",
        json!({
            "processId": Value::Null,
            "rootUri": Value::Null,
            "capabilities": {
                "workspace": { "configuration": true },
                "textDocument": text_document,
            },
        }),
    );
    let capabilities = server.response(id)["capabilities"].clone();
    server.notify("initialized", json!({}));
    (server, capabilities)
}

/// A component with something for each of the structure providers to find.
pub const STRUCTURED: &str = concat!(
    "<script>\n",
    "  import { onMount } from 'svelte';\n",
    "  import { get } from 'svelte/store';\n",
    "\n",
    "  let value = 1;\n",
    "</script>\n",
    "\n",
    "<!-- #region layout -->\n",
    "<div class=\"wrap\">\n",
    "  {#each [1, 2] as n}\n",
    "    <p title=\"row\">{n}</p>\n",
    "  {/each}\n",
    "</div>\n",
    "<!-- #endregion -->\n",
    "\n",
    "<style>\n",
    "  .wrap {\n",
    "    color: red;\n",
    "  }\n",
    "</style>\n",
);

pub fn did_open(server: &mut Server, uri: &str, text: &str) {
    server.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "svelte",
                "version": 1,
                "text": text,
            }
        }),
    );
}
