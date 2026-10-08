// @ts-check
// Orkestr Bridge — lets Orkestr run commands of VS Code extensions.
//
// Connects BACK to the Orkestr core on 127.0.0.1 (port + one-time token from
// the private file named by ORKESTR_BRIDGE_FILE) and answers newline-delimited
// JSON requests. It never listens on a port and talks to nothing else.
// MIT licensed. See README.md for the wire format.
"use strict";

const vscode = require("vscode");
const net = require("net");
const fs = require("fs");
const path = require("path");

const VERSION = "0.1.0";
const RETRY_MS = 2000;
const MAX_LINE = 4 * 1024 * 1024;
const MAX_RESULT = 64 * 1024;

/** @type {net.Socket | null} */
let socket = null;
/** @type {ReturnType<typeof setTimeout> | null} */
let timer = null;
let stopped = false;

/** @param {vscode.ExtensionContext} context */
function bridgeFile(context) {
  // Fallback: Orkestr keeps the file in the VS Code root, two levels above the extension.
  return process.env.ORKESTR_BRIDGE_FILE || path.join(context.extensionPath, "..", "..", "bridge.json");
}

/** @param {vscode.ExtensionContext} context */
function retry(context) {
  if (stopped || timer) return;
  timer = setTimeout(() => {
    timer = null;
    connect(context);
  }, RETRY_MS);
}

/** @param {unknown} value */
function serialize(value) {
  if (value === undefined) return null;
  try {
    const text = JSON.stringify(value);
    if (text === undefined) return null;
    if (text.length > MAX_RESULT) return { truncated: true, preview: text.slice(0, MAX_RESULT) };
    return JSON.parse(text);
  } catch {
    return String(value);
  }
}

/** Converts `{"$uri": "/abs/path"}` arguments to `vscode.Uri`. @param {unknown} arg @returns {unknown} */
function revive(arg) {
  if (Array.isArray(arg)) return arg.map(revive);
  if (arg && typeof arg === "object") {
    const obj = /** @type {Record<string, unknown>} */ (arg);
    if (typeof obj.$uri === "string") return vscode.Uri.file(obj.$uri);
    /** @type {Record<string, unknown>} */
    const out = {};
    for (const [k, v] of Object.entries(obj)) out[k] = revive(v);
    return out;
  }
  return arg;
}

/** @param {string} method @param {any} params */
async function handle(method, params) {
  switch (method) {
    case "ping":
      return { version: VERSION };
    case "listCommands":
      return await vscode.commands.getCommands(true);
    case "listExtensions":
      // Extensions of this (workspace) extension host and whether they are active.
      return vscode.extensions.all.map((e) => ({ id: e.id, active: e.isActive, kind: e.extensionKind }));
    case "execute": {
      if (typeof params?.command !== "string") throw new Error("command must be a string");
      const args = Array.isArray(params.args) ? params.args.map(revive) : [];
      return serialize(await vscode.commands.executeCommand(params.command, ...args));
    }
    case "openFile": {
      if (typeof params?.path !== "string") throw new Error("path must be a string");
      const line = Math.max(0, (Number(params.line) || 1) - 1);
      const col = Math.max(0, (Number(params.column) || 1) - 1);
      const pos = new vscode.Position(line, col);
      await vscode.window.showTextDocument(vscode.Uri.file(params.path), {
        selection: new vscode.Range(pos, pos),
        preview: false,
      });
      return null;
    }
    case "listTasks": {
      const tasks = await vscode.tasks.fetchTasks();
      return tasks.map((t) => ({ name: t.name, source: t.source, detail: t.detail ?? null }));
    }
    case "runTask": {
      const tasks = await vscode.tasks.fetchTasks();
      const task = tasks.find((t) => t.name === params?.name && (!params?.source || t.source === params.source));
      if (!task) throw new Error(`task not found: ${String(params?.name)}`);
      await vscode.tasks.executeTask(task);
      return null;
    }
    default:
      throw new Error(`unknown method: ${method}`);
  }
}

/** @param {net.Socket} s @param {unknown} msg */
function send(s, msg) {
  if (!s.destroyed) s.write(JSON.stringify(msg) + "\n");
}

/** @param {vscode.ExtensionContext} context */
function connect(context) {
  if (stopped) return;
  /** @type {{port?: unknown, token?: unknown} | null} */
  let info = null;
  try {
    info = JSON.parse(fs.readFileSync(bridgeFile(context), "utf8"));
  } catch {
    info = null;
  }
  if (!info || typeof info.port !== "number" || typeof info.token !== "string") {
    retry(context);
    return;
  }
  const token = info.token;
  const s = net.createConnection({ host: "127.0.0.1", port: info.port });
  socket = s;
  let buf = "";
  s.setEncoding("utf8");
  s.on("connect", () => {
    send(s, {
      type: "hello",
      token,
      version: VERSION,
      vscodeVersion: vscode.version,
      appName: vscode.env.appName,
      trusted: vscode.workspace.isTrusted,
      folders: (vscode.workspace.workspaceFolders || []).map((f) => f.uri.fsPath),
    });
  });
  s.on("data", (chunk) => {
    buf += chunk;
    if (buf.length > MAX_LINE) {
      s.destroy();
      return;
    }
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      /** @type {any} */
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      if (msg?.type !== "req") continue;
      const id = msg.id;
      handle(String(msg.method), msg.params).then(
        (result) => send(s, { type: "res", id, ok: true, result: result ?? null }),
        (e) => send(s, { type: "res", id, ok: false, error: String((e && e.message) || e) }),
      );
    }
  });
  s.on("error", () => undefined);
  s.on("close", () => {
    if (socket === s) socket = null;
    retry(context);
  });
}

/** @param {vscode.ExtensionContext} context */
function activate(context) {
  stopped = false;
  connect(context);
  context.subscriptions.push({
    dispose() {
      stopped = true;
      if (timer) clearTimeout(timer);
      timer = null;
      socket?.destroy();
    },
  });
}

function deactivate() {
  stopped = true;
  socket?.destroy();
}

module.exports = { activate, deactivate };
