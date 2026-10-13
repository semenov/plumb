// The Plumb extension: a small client for `plumb lsp` (JSON-RPC over the
// server's standard input and output), with no dependencies. It keeps the
// server's view of open .plumb files in sync and asks it for errors,
// completion, hover and definitions.

const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

let server = null;
let buffer = Buffer.alloc(0);
let nextId = 1;
const pending = new Map();
let diagnostics;
let output;

function findPlumb() {
  const configured = vscode.workspace.getConfiguration("plumb").get("path");
  if (configured) return configured;
  for (const dir of (process.env.PATH || "").split(path.delimiter)) {
    const p = path.join(dir, "plumb");
    if (dir && fs.existsSync(p)) return p;
  }
  const home = path.join(os.homedir(), ".plumb", "bin", "plumb");
  if (fs.existsSync(home)) return home;
  return null;
}

function send(msg) {
  if (!server) return;
  const body = Buffer.from(JSON.stringify(Object.assign({ jsonrpc: "2.0" }, msg)), "utf8");
  server.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
  server.stdin.write(body);
}

function request(method, params) {
  const id = nextId++;
  return new Promise(resolve => {
    pending.set(id, resolve);
    send({ id, method, params });
  });
}

function notify(method, params) {
  send({ method, params });
}

function onData(chunk) {
  buffer = Buffer.concat([buffer, chunk]);
  for (;;) {
    const sep = buffer.indexOf("\r\n\r\n");
    if (sep < 0) return;
    const header = buffer.slice(0, sep).toString("ascii");
    const m = /Content-Length:\s*(\d+)/i.exec(header);
    if (!m) { buffer = buffer.slice(sep + 4); continue; }
    const len = Number(m[1]);
    if (buffer.length < sep + 4 + len) return;
    const body = buffer.slice(sep + 4, sep + 4 + len).toString("utf8");
    buffer = buffer.slice(sep + 4 + len);
    let msg;
    try { msg = JSON.parse(body); } catch (e) { continue; }
    if (msg.id !== undefined && pending.has(msg.id)) {
      const resolve = pending.get(msg.id);
      pending.delete(msg.id);
      resolve(msg.error ? null : msg.result);
    } else if (msg.method === "textDocument/publishDiagnostics") {
      const uri = vscode.Uri.parse(msg.params.uri);
      diagnostics.set(uri, msg.params.diagnostics.map(d => {
        const diag = new vscode.Diagnostic(toRange(d.range), d.message, vscode.DiagnosticSeverity.Error);
        diag.source = "plumb";
        return diag;
      }));
    }
  }
}

const toRange = r => new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character);
const position = (doc, pos) => ({ textDocument: { uri: doc.uri.toString() }, position: { line: pos.line, character: pos.character } });
const isPlumb = doc => doc.languageId === "plumb" && doc.uri.scheme === "file";

function open(doc) {
  if (isPlumb(doc)) notify("textDocument/didOpen", { textDocument: { uri: doc.uri.toString(), languageId: "plumb", version: doc.version, text: doc.getText() } });
}

function start() {
  const bin = findPlumb();
  if (!bin) {
    vscode.window.showWarningMessage("Plumb: can't find the `plumb` program for errors and completion. Put it on the PATH, or set plumb.path.", "Open settings").then(choice => {
      if (choice) vscode.commands.executeCommand("workbench.action.openSettings", "plumb.path");
    });
    return;
  }
  const folder = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders[0];
  server = cp.spawn(bin, ["lsp"], { cwd: folder ? folder.uri.fsPath : os.homedir() });
  buffer = Buffer.alloc(0);
  server.stdout.on("data", onData);
  server.stderr.on("data", d => output.append(d.toString()));
  server.on("error", e => output.appendLine(`can't start ${bin}: ${e.message}`));
  server.on("exit", code => {
    output.appendLine(`plumb lsp exited (${code})`);
    server = null;
    for (const resolve of pending.values()) resolve(null);
    pending.clear();
  });
  request("initialize", { processId: process.pid, rootUri: folder ? folder.uri.toString() : null, capabilities: {} }).then(() => {
    notify("initialized", {});
    vscode.workspace.textDocuments.forEach(open);
  });
  output.appendLine(`started ${bin} lsp`);
}

function stop() {
  if (!server) return;
  const s = server;
  request("shutdown", null).then(() => { notify("exit", null); setTimeout(() => s.kill(), 500); });
}

const KINDS = vscode.CompletionItemKind;

function activate(context) {
  diagnostics = vscode.languages.createDiagnosticCollection("plumb");
  output = vscode.window.createOutputChannel("Plumb");
  context.subscriptions.push(diagnostics, output);
  start();

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(open),
    vscode.workspace.onDidChangeTextDocument(e => {
      if (isPlumb(e.document)) notify("textDocument/didChange", { textDocument: { uri: e.document.uri.toString(), version: e.document.version }, contentChanges: [{ text: e.document.getText() }] });
    }),
    vscode.workspace.onDidSaveTextDocument(doc => {
      if (isPlumb(doc)) notify("textDocument/didSave", { textDocument: { uri: doc.uri.toString() } });
    }),
    vscode.workspace.onDidCloseTextDocument(doc => {
      if (isPlumb(doc)) {
        notify("textDocument/didClose", { textDocument: { uri: doc.uri.toString() } });
        diagnostics.delete(doc.uri);
      }
    }),
    vscode.languages.registerCompletionItemProvider("plumb", {
      async provideCompletionItems(doc, pos) {
        const r = await request("textDocument/completion", position(doc, pos));
        if (!r) return [];
        return r.items.map(i => {
          const item = new vscode.CompletionItem(i.label, (i.kind || 1) - 1);
          item.detail = i.detail;
          if (i.documentation) item.documentation = new vscode.MarkdownString(i.documentation.value);
          if (i.insertText) item.insertText = i.insertTextFormat === 2 ? new vscode.SnippetString(i.insertText) : i.insertText;
          if (i.kind === 2 || i.kind === 3) item.sortText = "0" + i.label;
          return item;
        });
      },
    }, "."),
    vscode.languages.registerHoverProvider("plumb", {
      async provideHover(doc, pos) {
        const r = await request("textDocument/hover", position(doc, pos));
        if (!r) return null;
        return new vscode.Hover(new vscode.MarkdownString(r.contents.value), r.range ? toRange(r.range) : undefined);
      },
    }),
    vscode.languages.registerDefinitionProvider("plumb", {
      async provideDefinition(doc, pos) {
        const r = await request("textDocument/definition", position(doc, pos));
        if (!r) return null;
        return new vscode.Location(vscode.Uri.parse(r.uri), toRange(r.range));
      },
    }),
    vscode.commands.registerCommand("plumb.restart", () => { stop(); setTimeout(start, 700); }),
    vscode.workspace.onDidChangeConfiguration(e => {
      if (e.affectsConfiguration("plumb.path")) { stop(); setTimeout(start, 700); }
    }),
  );
}

function deactivate() {
  stop();
}

module.exports = { activate, deactivate };
