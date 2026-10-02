/**
 * Localizes `system.checks` results. The core sends English titles, details
 * and hints; the check ids are stable and the messages have a fixed shape
 * (crates/core/src/checks.rs), so the UI maps them to `settings:checks.*`.
 * Anything unknown (e.g. an OS error text inside a detail) is shown as sent.
 */
import type { SystemCheck } from "@orkestr/protocol";

import { formatNumber, t } from "../../../i18n";

const TITLE_IDS = ["git", "shell", "dataDir", "webview", "keyring"] as const;
type CheckId = (typeof TITLE_IDS)[number];

const isCheckId = (id: string): id is CheckId => (TITLE_IDS as readonly string[]).includes(id);

const STORES = {
  "Windows Credential Manager": "windows",
  "macOS Keychain": "macos",
  "Linux kernel keyring": "linux",
  "OS keyring": "other",
} as const;

function storeName(raw: string): string {
  const key = STORES[raw as keyof typeof STORES];
  return key ? t(`settings:checks.store.${key}`) : raw;
}

type HintKey =
  | "gitWindows"
  | "gitMac"
  | "gitLinux"
  | "shell"
  | "dataDirWritable"
  | "dataDirSpace"
  | "webviewWindows"
  | "webviewLinux"
  | "webviewHeadless"
  | "keyringFailed";

/** Exact hint texts of the core → key under `settings:checks.hint`. */
const HINTS: Record<string, HintKey> = {
  "Install Git for Windows (https://git-scm.com/download/win) or `winget install Git.Git`, then restart Orkestr.":
    "gitWindows",
  "Run `xcode-select --install` or `brew install git`, then restart Orkestr.": "gitMac",
  "Install git with your package manager (e.g. `sudo apt install git`), then restart Orkestr.": "gitLinux",
  "Install PowerShell 7 (`winget install Microsoft.PowerShell`) or make sure cmd/bash is available.": "shell",
  "Check folder permissions or set ORKESTR_HOME to a writable folder.": "dataDirWritable",
  "Free up disk space: snapshots and worktrees need room.": "dataDirSpace",
  "Install the Microsoft Edge WebView2 Runtime (https://go.microsoft.com/fwlink/p/?LinkId=2124703).": "webviewWindows",
  "Install WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`) for the desktop app.": "webviewLinux",
  "Not required for a headless core.": "webviewHeadless",
  "API keys cannot be stored securely until the credential store works; an encrypted file vault is used as fallback.":
    "keyringFailed",
};

function detailText(c: SystemCheck): string {
  const d = c.detail;
  let m: RegExpMatchArray | null;
  if (d === "check panicked") return t("settings:checks.detail.panicked");
  switch (c.id) {
    case "git":
      if (d === "git was not found on PATH") return t("settings:checks.detail.gitMissing");
      if ((m = d.match(/^git (.+) at (.+)$/))) return t("settings:checks.detail.gitFound", { version: m[1], path: m[2] });
      if ((m = d.match(/^`git --version` failed: ([\s\S]*)$/))) return t("settings:checks.detail.gitFailed", { error: m[1] });
      if ((m = d.match(/^git did not answer: ([\s\S]*)$/))) return t("settings:checks.detail.gitNoAnswer", { error: m[1] });
      return d;
    case "shell":
      if (d === "no supported shell was found") return t("settings:checks.detail.noShell");
      return d.replace(/ \(default\)/g, ` (${t("settings:checks.detail.defaultShell")})`);
    case "dataDir":
      if ((m = d.match(/^(.+) is not writable: ([\s\S]*)$/))) return t("settings:checks.detail.notWritable", { dir: m[1], error: m[2] });
      if ((m = d.match(/^(.+) — ([\d.]+) GB free$/))) {
        const free = `${formatNumber(Number(m[2]), { maximumFractionDigits: 1, minimumFractionDigits: 1 })} ${t("common:units.bytes.gb")}`;
        return t("settings:checks.detail.free", { dir: m[1], free });
      }
      return d;
    case "webview":
      if (d === "no web view runtime detected") return t("settings:checks.detail.noWebview");
      return d.replace("(system)", `(${t("settings:checks.detail.systemWebview")})`);
    case "keyring":
      if ((m = d.match(/^(.+) is available$/))) return t("settings:checks.detail.storeOk", { store: storeName(m[1] ?? "") });
      if ((m = d.match(/^(.+) is not supported$/))) return t("settings:checks.detail.storeUnsupported", { store: storeName(m[1] ?? "") });
      if ((m = d.match(/^(.+?) failed: ([\s\S]*)$/))) return t("settings:checks.detail.storeFailed", { store: storeName(m[1] ?? ""), error: m[2] });
      return d;
    default:
      return d;
  }
}

export interface CheckTexts {
  title: string;
  detail: string;
  hint?: string;
}

/** Title, detail and hint of a check in the current UI language. */
export function checkTexts(c: SystemCheck): CheckTexts {
  const title = isCheckId(c.id) ? t(`settings:checks.title.${c.id}`) : c.title;
  const out: CheckTexts = { title, detail: detailText(c) };
  if (c.hint) {
    const key = HINTS[c.hint];
    out.hint = key ? t(`settings:checks.hint.${key}`) : c.hint;
  }
  return out;
}
