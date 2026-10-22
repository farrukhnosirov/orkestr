/**
 * Marketplace panel (ADR-0040): Browse (search, kind / tag filters, details,
 * install with key trust + permission prompt), Installed (updates,
 * revocations, uninstall), Sources (official, directory, gateway registry)
 * and Keys (fingerprints, trust / untrust). Live via `MarketplaceUpdated`.
 */
import "./marketplace.css";

import type { Event } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Tabs } from "@orkestr/ui";
import { KeyRound, Package, Server, Store } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { maybeClient } from "../../core/client";
import { useT } from "../../i18n";
import type { PanelProps } from "../types";
import { BrowseTab } from "./BrowseTab";
import { useInstallFlow } from "./flow";
import { InstalledTab, KeysTab, SourcesTab } from "./tabs";

type MarketTab = "browse" | "installed" | "sources" | "keys";

export default function MarketplacePanel(_props: PanelProps) {
  const { t } = useT("marketplace");
  const [tab, setTab] = useState<MarketTab>("browse");
  const [version, setVersion] = useState(0);
  const bump = useCallback(() => setVersion((v) => v + 1), []);
  const flow = useInstallFlow(bump);

  useEffect(() => {
    const client = maybeClient();
    if (!client) return undefined;
    return client.subscribe([topic.global], (ev: Event) => {
      if (ev.kind === "MarketplaceUpdated") bump();
    });
  }, [bump]);

  const tabs = [
    {
      id: "browse" as const,
      label: t("tabs.browse"),
      icon: <Store size={14} />,
    },
    {
      id: "installed" as const,
      label: t("tabs.installed"),
      icon: <Package size={14} />,
    },
    {
      id: "sources" as const,
      label: t("tabs.sources"),
      icon: <Server size={14} />,
    },
    {
      id: "keys" as const,
      label: t("tabs.keys"),
      icon: <KeyRound size={14} />,
    },
  ];

  return (
    <div className="mp-panel">
      <div className="mp-header">
        <Tabs
          aria-label={t("tabs.label")}
          items={tabs}
          value={tab}
          onChange={setTab}
        />
      </div>
      {flow.notice && (
        <div
          className={`mp-notice mp-notice--${flow.notice.tone}`}
          role="status"
        >
          <span>{flow.notice.text}</span>
          <button
            type="button"
            className="mp-notice__close"
            aria-label={t("notice.dismiss")}
            onClick={() => flow.setNotice(null)}
          >
            ×
          </button>
        </div>
      )}
      <div className="mp-body">
        {tab === "browse" && <BrowseTab flow={flow} version={version} />}
        {tab === "installed" && (
          <InstalledTab flow={flow} version={version} onChanged={bump} />
        )}
        {tab === "sources" && (
          <SourcesTab flow={flow} version={version} onChanged={bump} />
        )}
        {tab === "keys" && (
          <KeysTab flow={flow} version={version} onChanged={bump} />
        )}
      </div>
      {flow.dialogs}
    </div>
  );
}
