/**
 * Local model store (ADR-0039): `models.list`, Import (mock model of a given
 * size, or a GGUF path) and Distribute to nodes (`models.distribute`).
 */
import type { DistributeOutcome, NodeSnapshot, StoredModel } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { formatBytes } from "../../i18n/format";
import { SettingGroup } from "../settings/controls";
import { NoticeLine, useAction } from "./useAction";

function ModelRow({ model, nodes, onChanged }: { model: StoredModel; nodes: NodeSnapshot[]; onChanged: () => Promise<void> }) {
  const { t } = useT("nodes");
  const { busy, notice, run } = useAction(onChanged);
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const [outcomes, setOutcomes] = useState<DistributeOutcome[]>([]);
  const ref = model.modelRef;
  const targets = nodes.filter((s) => !s.isCoordinator && s.status !== "Revoked");
  const name = (id: string) => nodes.find((s) => s.node.id === id)?.node.name ?? id.slice(0, 8);
  const toggle = (id: string) => setPicked((cur) => (cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]));

  const distribute = () =>
    run("distribute", async () => {
      const r = await getClient().call("models.distribute", { modelRef: ref, nodeIds: picked });
      setOutcomes(r.outcomes);
      setPicking(false);
      setPicked([]);
      const ok = r.outcomes.filter((o) => o.ok).length;
      return t("models.distributed", { ok, total: r.outcomes.length });
    });

  return (
    <li className="nd-card" data-testid={`model-${ref}`}>
      <div className="nd-card__head">
        <strong className="nd-card__name">{ref}</strong>
        <Badge tone="neutral">{model.manifest.format}</Badge>
        <span className="nd-muted">{formatBytes(model.sizeBytes)}</span>
        {model.ollamaInstalled && <Badge tone="success">{t("models.ollama")}</Badge>}
        <code className="nd-muted" title={model.digest}>
          {model.digest.slice(0, 19)}
        </code>
      </div>
      <div className="nd-actions">
        <Button size="sm" disabled={busy !== null || targets.length === 0} onClick={() => setPicking((v) => !v)} aria-expanded={picking} data-testid={`model-distribute-${ref}`}>
          {t("models.distribute")}
        </Button>
        {targets.length === 0 && <span className="nd-muted">{t("models.noTargets")}</span>}
      </div>
      {picking && (
        <fieldset className="nd-form" data-testid={`model-targets-${ref}`}>
          <legend>{t("models.pickNodes")}</legend>
          {targets.map((s) => (
            <label key={s.node.id} className="nd-check">
              <input type="checkbox" checked={picked.includes(s.node.id)} onChange={() => toggle(s.node.id)} />
              {s.node.name}
            </label>
          ))}
          <div className="nd-actions">
            <Button size="sm" variant="primary" loading={busy === "distribute"} disabled={busy !== null || picked.length === 0} onClick={() => void distribute()} data-testid={`model-distribute-confirm-${ref}`}>
              {t("models.send")}
            </Button>
          </div>
        </fieldset>
      )}
      {outcomes.length > 0 && (
        <ul className="nd-leases" aria-label={t("models.outcomes")}>
          {outcomes.map((o) => (
            <li key={o.nodeId}>
              {name(o.nodeId)} · {o.ok ? t("models.outcomeOk", { size: formatBytes(o.bytesSent) }) : t("models.outcomeFailed", { error: o.error ?? "" })}
            </li>
          ))}
        </ul>
      )}
      <NoticeLine notice={notice} />
    </li>
  );
}

export function ModelsTab({
  models,
  unavailable,
  nodes,
  onChanged,
}: {
  models: StoredModel[];
  unavailable: boolean;
  nodes: NodeSnapshot[];
  onChanged: () => Promise<void>;
}) {
  const { t } = useT("nodes");
  const { busy, notice, run } = useAction(onChanged);
  const [name, setName] = useState("");
  const [tag, setTag] = useState("latest");
  const [sizeMb, setSizeMb] = useState("20");
  const [path, setPath] = useState("");

  const importModel = () =>
    run("import", async () => {
      const mb = Number.parseFloat(sizeMb);
      const r = await getClient().call("models.import", {
        name: name.trim(),
        tag: tag.trim() || "latest",
        ...(path.trim() ? { path: path.trim() } : { mockSizeBytes: Math.max(1, Math.round((Number.isFinite(mb) ? mb : 1) * 1024 * 1024)) }),
      });
      setName("");
      setPath("");
      return t("models.imported", { ref: r.model.modelRef });
    });

  if (unavailable)
    return (
      <p className="nd-muted" role="status">
        {t("models.unavailable")}
      </p>
    );
  return (
    <div className="nd-stack">
      <SettingGroup title={t("models.importGroup")}>
        <p className="nd-muted">{t("models.importIntro")}</p>
        <div className="nd-form" data-testid="model-import-form">
          <Input label={t("models.name")} value={name} onChange={(e) => setName(e.target.value)} data-testid="model-import-name" />
          <Input label={t("models.tag")} value={tag} onChange={(e) => setTag(e.target.value)} />
          <Input label={t("models.path")} value={path} onChange={(e) => setPath(e.target.value)} hint={t("models.pathHint")} />
          <Input label={t("models.mockSize")} type="number" min={1} value={sizeMb} disabled={path.trim() !== ""} onChange={(e) => setSizeMb(e.target.value)} />
          <div className="nd-actions">
            <Button variant="primary" loading={busy === "import"} disabled={busy !== null || !name.trim()} onClick={() => void importModel()} data-testid="model-import-submit">
              {t("models.import")}
            </Button>
          </div>
        </div>
        <NoticeLine notice={notice} />
      </SettingGroup>
      {models.length === 0 ? (
        <p className="nd-muted">{t("models.none")}</p>
      ) : (
        <ul className="nd-list" aria-label={t("tabs.models")}>
          {models.map((m) => (
            <ModelRow key={m.digest} model={m} nodes={nodes} onChanged={onChanged} />
          ))}
        </ul>
      )}
    </div>
  );
}
