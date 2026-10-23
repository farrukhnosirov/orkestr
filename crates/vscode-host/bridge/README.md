# Orkestr Bridge

A tiny VS Code extension that Orkestr installs into its own embedded VS Code
(VSCodium REH web). It connects **back** to the Orkestr core over loopback
(`127.0.0.1`, port and one-time token from the private file named by
`ORKESTR_BRIDGE_FILE`) and answers requests from Orkestr:

- `listCommands` — `vscode.commands.getCommands(true)`
- `execute` — `vscode.commands.executeCommand(id, ...args)`
- `openFile` — open a file at a line/column
- `listTasks` / `runTask` — VS Code tasks
- `applyTheme` — Orkestr's theme as `workbench.colorCustomizations` scoped to
  a built-in base theme (the color theme itself is only switched while a
  built-in default theme is selected, so a theme extension the user picked
  stays)

Two keybindings send a `signal` back to Orkestr (VS Code embedded in the
Orkestr window keeps the keyboard while it has focus):

- `Ctrl+Alt+K` — `orkestr.showPalette`: open Orkestr's command palette
- `Ctrl+Alt+O` — `orkestr.focusOrkestr`: return keyboard focus to Orkestr

It opens no port itself and makes no other network connection. Every request
Orkestr sends is audit-logged on the Orkestr side; commands that look
destructive need a confirmation in Orkestr first.

The wire format is newline-delimited JSON:

```
→ {"type":"hello","token":"…","version":"0.2.0","vscodeVersion":"1.x"}
← {"type":"welcome"}
← {"type":"req","id":1,"method":"execute","params":{"command":"…","args":[]}}
→ {"type":"res","id":1,"ok":true,"result":…}
→ {"type":"signal","signal":"showPalette"}
```

`extension.js` is plain JavaScript checked with `// @ts-check` so the bundle
needs no build step; Orkestr packs it into a `.vsix` at runtime. MIT licensed.
