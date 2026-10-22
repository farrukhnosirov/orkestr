# @orkestr/widget-sdk

Bridge for **plugin dashboard widgets** (ADR-0034). A plugin widget is one
self-contained HTML file declared in the plugin manifest (`widgets[]`). The
desktop loads it into

```html
<iframe sandbox="allow-scripts" srcdoc="…">   <!-- no allow-same-origin -->
```

with this CSP injected:

```
default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:;
connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; worker-src 'none';
form-action 'none'; base-uri 'none'
```

So a widget has an opaque origin, no `__TAURI_INTERNALS__`, no network, no
storage of the app. Inline your scripts and styles (bundle the SDK into the
HTML). The only channel is `postMessage`:

| Host → widget | Widget → host |
| --- | --- |
| `init {widgetId, instanceId, config, theme, locale}` | `ready` |
| `data {bindingId, result}` (query reply or pushed data) | `query {reqId, method, params}` |
| `event {kind, payload}` | `subscribe {kinds}` |
| `theme {theme}` | `resize {size: {w, h}}` |
| `error {reqId, error: {code, message}}` | |

Data access is limited to the widget's manifest `dataBinding`: methods must be
**Read-class and not local-only** (checked when the bundle is served, by the
host bridge on every query, and by core); event kinds must be listed in
`dataBinding.events`. Anything else is answered with `error` (`forbidden`).

## Minimal widget

Manifest excerpt:

```json
{
  "kinds": ["widget"],
  "widgets": [{
    "id": "alerts-count",
    "title": "Open alerts",
    "entryHtml": "widgets/alerts.html",
    "sizes": [{ "w": 3, "h": 2 }],
    "dataBinding": { "methods": ["monitor.alerts"], "events": ["AlertRaised", "AlertResolved"] },
    "configSchema": {
      "type": "object",
      "properties": { "label": { "type": "string", "title": "Label", "maxLength": 40 } },
      "additionalProperties": false
    }
  }]
}
```

`widgets/alerts.html` (the SDK bundled inline, e.g. with esbuild `--bundle --format=iife`):

```html
<!doctype html>
<html>
  <body style="margin:0;font:13px var(--ok-font-sans);color:var(--ok-color-text);background:transparent">
    <strong id="label">Open alerts</strong> <span id="n">…</span>
    <script>
      // import { createWidget } from "@orkestr/widget-sdk";
      const widget = createWidget({
        onInit: ({ config }) => {
          if (config && config.label) document.getElementById("label").textContent = config.label;
          refresh();
        },
        onEvent: () => refresh(),
      });
      widget.subscribe(["AlertRaised", "AlertResolved"]);
      async function refresh() {
        try {
          const res = await widget.query("monitor.alerts", {});
          document.getElementById("n").textContent = String(res.alerts.length);
        } catch (e) {
          document.getElementById("n").textContent = e.code; // e.g. "forbidden"
        }
      }
    </script>
  </body>
</html>
```

Theme tokens arrive in `init`/`theme` and are applied as `--ok-*` CSS
variables on `<html>` (disable with `applyTheme: false`).

## API

- `createWidget({ onInit, onData?, onEvent?, onTheme?, timeoutMs?, autoReady?, applyTheme? })`
  → `{ query(method, params), subscribe(kinds), resize({w, h}), ready(), dispose() }`.
  `query` is typed with `@orkestr/protocol` (`MethodName` / `ParamsOf` / `ResultOf`)
  and rejects with `WidgetQueryError { code, message }`.
- `parseHostMessage(data)`, `applyThemeTokens(theme)` — helpers.

`npm test -w @orkestr/widget-sdk` runs the unit tests.
