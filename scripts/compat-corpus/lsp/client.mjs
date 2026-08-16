#!/usr/bin/env node
/**
 * A minimal LSP client over stdio, used to drive BOTH the official
 * `svelte-language-server` and `rsvelte-language-server` with the same request
 * stream (see `lsp-verify.mjs`).
 *
 * Upstream ships no end-to-end protocol test — every `language-server` test
 * calls a plugin class directly — so the transport had to be written here
 * rather than reused.
 */

import { spawn } from 'node:child_process';

const HEADER_END = '\r\n\r\n';

/** Thrown when a server answers a request with a JSON-RPC `error` object. */
export class LspError extends Error {
	constructor(method, error) {
		super(`${method}: ${error.message} (code ${error.code})`);
		this.name = 'LspError';
		this.code = error.code;
	}
}

/** Thrown when a request outlives its deadline; the server is left running. */
export class LspTimeout extends Error {
	constructor(method, ms) {
		super(`${method}: no response within ${ms}ms`);
		this.name = 'LspTimeout';
	}
}

export class LspClient {
	/**
	 * @param {object} options
	 * @param {string} options.command  executable to spawn
	 * @param {string[]} options.args
	 * @param {string} options.cwd
	 * @param {Record<string,string>} [options.env]
	 * @param {string} options.label  used in error messages and traces
	 * @param {number} [options.timeoutMs]
	 * @param {boolean} [options.trace]  echo stderr to this process's stderr
	 */
	constructor({ command, args = [], cwd, env = {}, label, timeoutMs = 60_000, trace = false }) {
		this.label = label;
		this.timeoutMs = timeoutMs;
		this.proc = spawn(command, args, {
			cwd,
			stdio: ['pipe', 'pipe', 'pipe'],
			env: { ...process.env, ...env },
		});
		this.nextId = 1;
		this.pending = new Map();
		/** Every `textDocument/publishDiagnostics` seen, newest last, keyed by URI. */
		this.diagnostics = new Map();
		/** Server-initiated requests get a canned reply; unanswered ones deadlock the server. */
		this.serverRequests = [];
		this.notifications = [];
		this.stderr = '';
		this.exited = null;
		this.buffer = Buffer.alloc(0);

		this.proc.stdout.on('data', (chunk) => this.#onData(chunk));
		this.proc.stderr.on('data', (chunk) => {
			this.stderr += chunk.toString('utf8');
			// Unbounded stderr from a chatty server is a memory leak in long sweeps.
			if (this.stderr.length > 1 << 20) this.stderr = this.stderr.slice(-(1 << 19));
			if (trace) process.stderr.write(`[${label}] ${chunk}`);
		});
		this.proc.on('exit', (code, signal) => {
			this.exited = { code, signal };
			for (const [, p] of this.pending) {
				p.reject(new Error(`${label} exited (code ${code}, signal ${signal})`));
			}
			this.pending.clear();
		});
		this.proc.on('error', (err) => {
			this.exited = { code: null, signal: null, error: err };
		});
	}

	#onData(chunk) {
		this.buffer = Buffer.concat([this.buffer, chunk]);
		for (;;) {
			const headerEnd = this.buffer.indexOf(HEADER_END);
			if (headerEnd < 0) return;
			const header = this.buffer.subarray(0, headerEnd).toString('ascii');
			const match = /content-length:\s*(\d+)/i.exec(header);
			if (!match) {
				// Unrecoverable: without a length we cannot find the next frame.
				this.buffer = Buffer.alloc(0);
				return;
			}
			const length = Number(match[1]);
			const start = headerEnd + HEADER_END.length;
			if (this.buffer.length < start + length) return;
			const body = this.buffer.subarray(start, start + length).toString('utf8');
			this.buffer = this.buffer.subarray(start + length);
			let message;
			try {
				message = JSON.parse(body);
			} catch {
				continue;
			}
			this.#dispatch(message);
		}
	}

	#dispatch(message) {
		if (message.id !== undefined && message.method === undefined) {
			const pending = this.pending.get(message.id);
			if (!pending) return;
			this.pending.delete(message.id);
			clearTimeout(pending.timer);
			if (message.error) pending.reject(new LspError(pending.method, message.error));
			else pending.resolve(message.result ?? null);
			return;
		}
		if (message.id !== undefined) {
			this.serverRequests.push(message);
			this.#answerServerRequest(message);
			return;
		}
		if (message.method === 'textDocument/publishDiagnostics') {
			this.diagnostics.set(message.params.uri, message.params.diagnostics);
		}
		this.notifications.push(message);
		if (this.notifications.length > 5000) this.notifications.splice(0, 2500);
	}

	/**
	 * Both servers ask the client for things during `initialize`. Refusing to
	 * answer stalls them, so every server request gets a shape-correct reply.
	 */
	#answerServerRequest(message) {
		let result = null;
		if (message.method === 'workspace/configuration') {
			result = (message.params?.items ?? []).map(() => null);
		} else if (message.method === 'client/registerCapability') {
			result = null;
		} else if (message.method === 'client/unregisterCapability') {
			result = null;
		} else if (message.method === 'window/workDoneProgress/create') {
			result = null;
		} else if (message.method === 'workspace/applyEdit') {
			result = { applied: false };
		}
		this.#send({ jsonrpc: '2.0', id: message.id, result });
	}

	#send(message) {
		if (this.exited) return;
		const body = Buffer.from(JSON.stringify(message), 'utf8');
		this.proc.stdin.write(`Content-Length: ${body.length}${HEADER_END}`);
		this.proc.stdin.write(body);
	}

	notify(method, params) {
		this.#send({ jsonrpc: '2.0', method, params });
	}

	request(method, params, { timeoutMs = this.timeoutMs } = {}) {
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new LspTimeout(`${this.label}/${method}`, timeoutMs));
			}, timeoutMs);
			timer.unref?.();
			this.pending.set(id, { resolve, reject, timer, method: `${this.label}/${method}` });
			this.#send({ jsonrpc: '2.0', id, method, params });
		});
	}

	/** Send `$/cancelRequest` for an id that was already handed out. */
	cancel(id) {
		this.notify('$/cancelRequest', { id });
	}

	/** Wait until `predicate` holds or the deadline passes. Returns whether it held. */
	async waitFor(predicate, { timeoutMs = this.timeoutMs, intervalMs = 20 } = {}) {
		const deadline = Date.now() + timeoutMs;
		for (;;) {
			if (predicate()) return true;
			if (Date.now() > deadline || this.exited) return predicate();
			await new Promise((r) => setTimeout(r, intervalMs));
		}
	}

	async shutdown({ timeoutMs = 5_000 } = {}) {
		if (this.exited) return;
		try {
			await this.request('shutdown', null, { timeoutMs });
			this.notify('exit', null);
		} catch {
			// A server that will not shut down cleanly still has to die.
		}
		const gone = await this.waitFor(() => this.exited !== null, { timeoutMs, intervalMs: 10 });
		if (!gone) this.proc.kill('SIGKILL');
	}

	kill(signal = 'SIGKILL') {
		this.proc.kill(signal);
	}
}

/** The client capabilities both servers are initialized with — identical by construction. */
export function clientCapabilities() {
	const markup = { contentFormat: ['markdown', 'plaintext'] };
	return {
		general: { positionEncodings: ['utf-16'] },
		workspace: {
			workspaceFolders: true,
			configuration: true,
			applyEdit: true,
			didChangeWatchedFiles: { dynamicRegistration: true },
			symbol: { symbolKind: { valueSet: range(1, 26) } },
			executeCommand: {},
		},
		textDocument: {
			synchronization: { dynamicRegistration: true, didSave: true },
			publishDiagnostics: { relatedInformation: true, tagSupport: { valueSet: [1, 2] } },
			completion: {
				completionItem: {
					snippetSupport: true,
					documentationFormat: markup.contentFormat,
					insertReplaceSupport: true,
					labelDetailsSupport: true,
					resolveSupport: { properties: ['documentation', 'detail', 'additionalTextEdits'] },
				},
				completionItemKind: { valueSet: range(1, 25) },
				contextSupport: true,
			},
			hover: markup,
			signatureHelp: { signatureInformation: { documentationFormat: markup.contentFormat } },
			definition: { linkSupport: true },
			typeDefinition: { linkSupport: true },
			implementation: { linkSupport: true },
			references: {},
			documentHighlight: {},
			documentSymbol: {
				symbolKind: { symbolKind: { valueSet: range(1, 26) } },
				hierarchicalDocumentSymbolSupport: true,
			},
			codeAction: {
				codeActionLiteralSupport: {
					codeActionKind: {
						valueSet: ['', 'quickfix', 'refactor', 'refactor.extract', 'source', 'source.organizeImports'],
					},
				},
			},
			formatting: {},
			rangeFormatting: {},
			rename: { prepareSupport: true },
			foldingRange: { lineFoldingOnly: true },
			selectionRange: {},
			semanticTokens: {
				requests: { full: true, range: true },
				tokenTypes: [],
				tokenModifiers: [],
				formats: ['relative'],
			},
			inlayHint: { resolveSupport: { properties: ['tooltip'] } },
			linkedEditingRange: {},
			colorProvider: {},
			documentLink: {},
			codeLens: {},
		},
	};
}

function range(from, to) {
	return Array.from({ length: to - from + 1 }, (_, i) => from + i);
}
