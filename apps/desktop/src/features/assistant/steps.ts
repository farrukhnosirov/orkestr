/**
 * Field specs of the action vocabulary for the command editor: each step
 * type → its editable fields. Mirrors `ComputerAction` (generated from Rust).
 */
import type { ComputerAction } from "@orkestr/protocol";

export type ActionType = ComputerAction["type"];

export type FieldKind = "text" | "num" | "bool" | "json" | { options: readonly string[] };

export interface FieldSpec {
  key: string;
  kind: FieldKind;
  optional?: boolean;
}

const WINDOW_OPS = [
  "Focus", "Minimize", "Maximize", "Restore", "Close", "SnapLeft", "SnapRight", "SnapTop", "SnapBottom", "SnapTopLeft", "SnapTopRight",
  "SnapBottomLeft", "SnapBottomRight", "Center", "AlwaysOnTop", "NotOnTop", "NextMonitor", "PreviousMonitor", "SwitchNext", "ShowDesktop",
  "MinimizeAll", "RestoreAll",
] as const;
const LEVEL_OPS = ["Up", "Down", "Set", "Mute", "MuteOn", "MuteOff", "Scale"] as const;
const DIRECTIONS = ["Up", "Down", "Left", "Right"] as const;
const BUTTONS = ["Left", "Right", "Middle", "Double"] as const;

export const STEP_FIELDS: Record<ActionType, readonly FieldSpec[]> = {
  Launch: [{ key: "target", kind: "text" }],
  OpenUrl: [{ key: "url", kind: "text" }],
  WebSearch: [
    { key: "query", kind: "text" },
    { key: "engine", kind: "text", optional: true },
  ],
  OpenSettings: [{ key: "page", kind: "text" }],
  OpenPath: [{ key: "path", kind: "text" }],
  CloseApp: [
    { key: "app", kind: "text" },
    { key: "discard", kind: "bool" },
  ],
  KillProcess: [{ key: "name", kind: "text" }],
  FocusApp: [{ key: "app", kind: "text" }],
  Window: [
    { key: "op", kind: { options: WINDOW_OPS } },
    { key: "app", kind: "text", optional: true },
  ],
  Keys: [
    { key: "combo", kind: "text" },
    { key: "times", kind: "num", optional: true },
  ],
  TypeText: [{ key: "text", kind: "text" }],
  Scroll: [
    { key: "direction", kind: { options: ["Up", "Down", "Left", "Right"] } },
    { key: "amount", kind: "num", optional: true },
    { key: "unit", kind: { options: ["Lines", "Pages", "Edge"] } },
  ],
  ClickElement: [{ key: "name", kind: "text" }],
  MouseClick: [{ key: "button", kind: { options: BUTTONS } }],
  Media: [{ key: "key", kind: { options: ["PlayPause", "Next", "Previous", "Stop"] } }],
  Volume: [
    { key: "op", kind: { options: LEVEL_OPS } },
    { key: "value", kind: "num", optional: true },
    { key: "app", kind: "text", optional: true },
  ],
  Microphone: [
    { key: "op", kind: { options: LEVEL_OPS } },
    { key: "value", kind: "num", optional: true },
  ],
  AudioDevice: [
    { key: "kind", kind: { options: ["Output", "Input"] } },
    { key: "name", kind: "text", optional: true },
  ],
  MediaSeek: [
    { key: "forward", kind: "bool" },
    { key: "seconds", kind: "num", optional: true },
  ],
  Brightness: [
    { key: "op", kind: { options: ["Up", "Down", "Set", "Scale"] } },
    { key: "value", kind: "num", optional: true },
  ],
  WindowResize: [
    { key: "app", kind: "text", optional: true },
    { key: "percent", kind: "num", optional: true },
    { key: "width", kind: "num", optional: true },
    { key: "height", kind: "num", optional: true },
  ],
  WindowOpacity: [
    { key: "app", kind: "text", optional: true },
    { key: "percent", kind: "num" },
  ],
  Desktop: [{ key: "op", kind: { options: ["Next", "Previous", "New", "Close", "TaskView"] } }],
  MouseMove: [
    { key: "direction", kind: { options: DIRECTIONS } },
    { key: "distance", kind: "num", optional: true },
  ],
  MouseTo: [{ key: "point", kind: { options: ["Center", "TopLeft", "TopRight", "BottomLeft", "BottomRight", "Top", "Bottom", "Left", "Right"] } }],
  MouseToElement: [{ key: "name", kind: "text" }],
  MouseHold: [
    { key: "button", kind: { options: BUTTONS } },
    { key: "release", kind: "bool" },
  ],
  MouseDrag: [
    { key: "direction", kind: { options: DIRECTIONS }, optional: true },
    { key: "distance", kind: "num", optional: true },
    { key: "element", kind: "text", optional: true },
  ],
  MouseSpeed: [
    { key: "op", kind: { options: ["Up", "Down", "Set", "Scale"] } },
    { key: "value", kind: "num", optional: true },
  ],
  LocateCursor: [],
  KeyboardLayout: [{ key: "lang", kind: "text", optional: true }],
  Display: [{ key: "op", kind: { options: ["ScreenOff", "PcScreenOnly", "Duplicate", "Extend", "SecondScreenOnly"] } }],
  Radio: [
    { key: "kind", kind: { options: ["Wifi", "Bluetooth", "All"] } },
    { key: "on", kind: "bool" },
  ],
  Query: [
    {
      key: "what",
      kind: {
        options: [
          "DateTime", "Date", "Time", "Battery", "DiskSpace", "IpAddress", "Network", "SystemInfo", "Volume", "NowPlaying",
          "ActiveWindow", "CursorPosition", "AudioDevices", "AudioSessions", "Brightness", "Uptime", "KeyboardLayout",
        ],
      },
    },
  ],
  Calculate: [{ key: "expression", kind: "text" }],
  Alarm: [
    { key: "at", kind: "text" },
    { key: "message", kind: "text" },
  ],
  FileCopy: [
    { key: "from", kind: "text" },
    { key: "to", kind: "text" },
  ],
  FileSearch: [
    { key: "name", kind: "text" },
    { key: "folder", kind: "text", optional: true },
  ],
  EmptyRecycleBin: [],
  System: [{ key: "op", kind: { options: ["Lock", "Sleep", "Shutdown", "Restart", "SignOut"] } }],
  Screenshot: [
    { key: "path", kind: "text", optional: true },
    { key: "scope", kind: { options: ["Full", "Window", "Region"] } },
    { key: "clipboard", kind: "bool" },
  ],
  ClipboardSet: [{ key: "text", kind: "text" }],
  ClipboardGet: [],
  Notify: [
    { key: "title", kind: "text" },
    { key: "body", kind: "text" },
  ],
  Timer: [
    { key: "seconds", kind: "num" },
    { key: "message", kind: "text" },
  ],
  ListProcesses: [],
  ListWindows: [],
  FileCreate: [
    { key: "path", kind: "text" },
    { key: "content", kind: "text" },
  ],
  FolderCreate: [{ key: "path", kind: "text" }],
  FileMove: [
    { key: "from", kind: "text" },
    { key: "to", kind: "text" },
  ],
  FileDelete: [{ key: "path", kind: "text" }],
  Wait: [{ key: "ms", kind: "num" }],
  Speak: [{ key: "text", kind: "text" }],
  SetMode: [{ key: "mode", kind: { options: ["Orchestra", "Assistant"] } }],
  If: [
    { key: "condition", kind: "json" },
    { key: "then", kind: "json" },
    { key: "otherwise", kind: "json" },
  ],
};

export const ACTION_TYPES = Object.keys(STEP_FIELDS) as ActionType[];

/** A new step of `type` with default values. */
export function defaultStep(type: ActionType): ComputerAction {
  const step: Record<string, unknown> = { type };
  for (const f of STEP_FIELDS[type]) {
    if (f.optional) continue;
    if (f.kind === "text") step[f.key] = "";
    else if (f.kind === "num") step[f.key] = f.key === "ms" ? 1000 : f.key === "seconds" ? 60 : 1;
    else if (f.kind === "bool") step[f.key] = false;
    else if (f.kind === "json") step[f.key] = f.key === "condition" ? { type: "WindowExists", app: "" } : [];
    else step[f.key] = f.kind.options[0];
  }
  return step as ComputerAction;
}

/** Number-or-slot field value from an input (`"5"` → 5, `"{n}"` stays text). */
export function slotNum(raw: string): number | string {
  const n = Number(raw);
  return raw.trim() !== "" && Number.isFinite(n) && n >= 0 ? Math.round(n) : raw;
}
