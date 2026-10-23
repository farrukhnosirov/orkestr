/**
 * Settings → Voice → Models (ADR-0046): the voice package catalog with
 * download (with progress), cancel, remove and checksum verification, plus
 * the disk usage of `<ORKESTR_HOME>/voice/models`. Polls while a download
 * is queued or running.
 */
import type { VoiceModelsResult, VoicePackageInfo } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { Download, ShieldCheck, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { maybeClient } from "../../../core/client";
import { describeError } from "../../../i18n/errors";
import { useFormat, useT } from "../../../i18n";

const ACTIVE = new Set(["Queued", "Downloading", "Installing"]);
const STATE_TONE = { NotInstalled: "neutral", Queued: "info", Downloading: "info", Installing: "info", Installed: "success", Failed: "danger" } as const;

export function ModelsPanel({ pollMs = 1000 }: { pollMs?: number }) {
  const { t } = useT(["voice", "common"]);
  const fmt = useFormat();
  const [data, setData] = useState<VoiceModelsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<Record<string, string>>({});

  const call = useCallback(async (method: "voice.listModels" | "voice.downloadModel" | "voice.cancelDownload" | "voice.removeModel", id?: string) => {
    const client = maybeClient();
    if (!client) return;
    try {
      const r = method === "voice.listModels" ? await client.call(method, {}) : await client.call(method, { id: id ?? "" });
      setData(r);
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  useEffect(() => {
    void call("voice.listModels");
  }, [call]);

  const active = data?.packages.some((p) => ACTIVE.has(p.state)) ?? false;
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => void call("voice.listModels"), pollMs);
    return () => window.clearInterval(id);
  }, [active, call, pollMs]);

  const verify = async (p: VoicePackageInfo) => {
    const client = maybeClient();
    if (!client) return;
    try {
      const r = await client.call("voice.verifyModel", { id: p.id }, { timeoutMs: 300_000 });
      setNotes((n) => ({ ...n, [p.id]: r.ok ? t("settings.models.verified", { count: r.files }) : t("settings.models.verifyFailed", { count: r.mismatches.length }) }));
    } catch (e) {
      setNotes((n) => ({ ...n, [p.id]: describeError(e) }));
    }
  };

  if (!data) return error ? <div className="set-alert">{error}</div> : <Spinner size={16} />;

  return (
    <div className="vx-models">
      <div className="set-muted set-muted--small">{t("settings.models.diskUsage", { size: fmt.bytes(data.diskUsageBytes), path: data.root })}</div>
      {error && <div className="set-alert">{error}</div>}
      <ul className="vx-models__list">
        {data.packages.map((p) => {
          const pct = p.downloadBytes > 0 ? Math.min(1, p.progressBytes / p.downloadBytes) : 0;
          return (
            <li key={p.id} className="vx-model" data-testid={`model-${p.id}`}>
              <div className="vx-model__main">
                <strong>{p.title}</strong>
                <div className="vx-engine__chips">
                  <Badge tone={STATE_TONE[p.state]}>{t(`settings.models.state.${p.state}`)}</Badge>
                  <Badge>{t(`settings.models.kind.${p.kind}`)}</Badge>
                  {p.recommended && <Badge tone="accent">{t("settings.models.recommended")}</Badge>}
                  <span className="set-muted set-muted--small">
                    {p.state === "Installed" ? t("settings.models.onDisk", { size: fmt.bytes(p.installedBytes) }) : t("settings.models.download", { size: fmt.bytes(p.downloadBytes) })}
                    {" · "}
                    {p.source} · {p.license}
                  </span>
                </div>
                {(p.state === "Downloading" || p.state === "Installing") && (
                  <div className="vx-model__bar" role="progressbar" aria-label={p.title} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct * 100)}>
                    <span style={{ width: `${Math.round(pct * 100)}%` }} />
                  </div>
                )}
                {p.error && <div className="set-alert">{p.error}</div>}
                {notes[p.id] && <div className="set-muted set-muted--small" role="status">{notes[p.id]}</div>}
                {!p.supported && <div className="set-muted set-muted--small">{t("settings.models.unsupported")}</div>}
              </div>
              <div className="vx-model__actions">
                {(p.state === "NotInstalled" || p.state === "Failed") && (
                  <Button size="sm" disabled={!p.supported} onClick={() => void call("voice.downloadModel", p.id)}>
                    <Download size={14} /> {t("settings.models.downloadButton")}
                  </Button>
                )}
                {ACTIVE.has(p.state) && (
                  <Button size="sm" variant="ghost" onClick={() => void call("voice.cancelDownload", p.id)}>
                    <X size={14} /> {t("common:actions.cancel")}
                  </Button>
                )}
                {p.state === "Installed" && (
                  <>
                    <Button size="sm" variant="ghost" onClick={() => void verify(p)}>
                      <ShieldCheck size={14} /> {t("settings.models.verify")}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => void call("voice.removeModel", p.id)}>
                      <Trash2 size={14} /> {t("settings.models.remove")}
                    </Button>
                  </>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
