/**
 * Sandboxed plugin widget (ADR-0034): `<iframe sandbox="allow-scripts">`
 * (never `allow-same-origin`) with a `srcdoc` built from `widgets.bundle`
 * and the injected CSP. The host bridge validates every message (source +
 * opaque origin) and the binding allowlist before calling the RPC client.
 */
import type { WidgetDescriptor } from "@orkestr/protocol";
import type { WidgetGridSize } from "@orkestr/widget-sdk";
import { Spinner } from "@orkestr/ui";
import { useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { currentLanguage, useT } from "../../i18n";
import { useRpcQuery } from "../../lib/rpcQuery";
import { type BridgeClient, createWidgetBridge } from "./bridge";
import { buildWidgetDocument, readThemeTokens, WIDGET_CSP, WIDGET_SANDBOX, widgetDocumentLoader } from "./frame";

export interface WidgetFrameProps {
  descriptor: Pick<WidgetDescriptor, "id" | "title" | "dataBinding">;
  instanceId: string;
  config: unknown;
  /** Event topics visible to the widget (`global`, `project:<id>`). */
  topics: readonly string[];
  onResize?: (size: WidgetGridSize) => void;
  /** Test seam (defaults to the core client). */
  client?: BridgeClient;
}

const coreBridgeClient: BridgeClient = {
  call: (method, params) => getClient().call(method as never, params as never),
  subscribe: (topics, handler) => getClient().subscribe(topics, handler),
};

export function WidgetFrame({ descriptor, instanceId, config, topics, onResize, client }: WidgetFrameProps) {
  const { t } = useT("dashboards");
  const ref = useRef<HTMLIFrameElement>(null);
  const bundle = useRpcQuery(`widgets.bundle:${descriptor.id}`, (c) => c.call("widgets.bundle", { widgetId: descriptor.id }));
  // The host's own CSP constant is authoritative; the core one is informative.
  const srcdoc = useMemo(() => (bundle.data ? buildWidgetDocument(bundle.data.html, WIDGET_CSP) : null), [bundle.data]);
  // URL mode (app shell loader, see `setWidgetDocumentLoader`); srcdoc otherwise.
  const [src, setSrc] = useState<{ doc: string; url: string | null; error: string | null } | null>(null);
  useEffect(() => {
    const loader = widgetDocumentLoader();
    if (!srcdoc || !loader || !bundle.data) return undefined;
    let cancelled = false;
    loader({ widgetId: descriptor.id, instanceId, html: bundle.data.html, csp: WIDGET_CSP })
      .then((url) => !cancelled && setSrc({ doc: srcdoc, url, error: null }))
      .catch((e: unknown) => !cancelled && setSrc({ doc: srcdoc, url: null, error: String(e) }));
    return () => {
      cancelled = true;
    };
  }, [srcdoc, bundle.data, descriptor.id, instanceId]);
  const urlMode = widgetDocumentLoader() !== null;
  const frameUrl = urlMode && src?.doc === srcdoc ? src.url : null;
  const onResizeRef = useRef(onResize);
  onResizeRef.current = onResize;
  const topicsKey = topics.join(",");
  const configKey = JSON.stringify(config ?? null);
  const bindingKey = JSON.stringify(descriptor.dataBinding);

  useEffect(() => {
    if (!srcdoc) return undefined;
    const bridge = createWidgetBridge({
      widgetId: descriptor.id,
      instanceId,
      dataBinding: JSON.parse(bindingKey) as WidgetFrameProps["descriptor"]["dataBinding"],
      config: JSON.parse(configKey) as unknown,
      locale: currentLanguage(),
      topics: topicsKey ? topicsKey.split(",") : [],
      theme: () => readThemeTokens(),
      target: () => ref.current?.contentWindow ?? null,
      client: client ?? coreBridgeClient,
      onResize: (size) => onResizeRef.current?.(size),
    });
    const listener = (e: MessageEvent) => bridge.handle(e);
    window.addEventListener("message", listener);
    const observer = new MutationObserver(() => bridge.sendTheme());
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style", "data-theme"] });
    // Config changed while loaded: re-init (a fresh frame asks with `ready`).
    bridge.sendInit();
    return () => {
      window.removeEventListener("message", listener);
      observer.disconnect();
      bridge.dispose();
    };
  }, [srcdoc, descriptor.id, bindingKey, instanceId, configKey, topicsKey, client]);

  const loadError = bundle.error ?? (urlMode && src?.doc === srcdoc ? src.error : null);
  if (loadError) return <div className="mc-error" role="alert">{loadError}</div>;
  if (!srcdoc || (urlMode && !frameUrl)) {
    return (
      <div className="mc-empty">
        <Spinner size={16} label={t("widget.loading")} />
      </div>
    );
  }
  return (
    <iframe
      ref={ref}
      className="dsh-frame"
      title={descriptor.title}
      sandbox={WIDGET_SANDBOX}
      {...(frameUrl ? { src: frameUrl } : { srcDoc: srcdoc })}
      referrerPolicy="no-referrer"
      loading="lazy"
      data-widget-frame={descriptor.id}
    />
  );
}
