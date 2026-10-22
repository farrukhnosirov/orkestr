/**
 * Publish tab (ADR-0040): sign a local `.orkp` package or agent template JSON
 * with this installation's vault-held publisher key and add it to a writable
 * source (folder registry or gateway). The official source is read-only.
 */
import "../../features/plugins/plugins.css";

import { Badge, Button, Input } from "@orkestr/ui";
import { KeyRound, Upload } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { PathInput } from "../../components/PathInput";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import {
  type MarketEntry,
  type PackageKind,
  type PublisherKey,
  type RegistrySource,
  marketplaceApi,
} from "./api";
import { PACKAGE_KINDS, fingerprintRows } from "./model";

/** Sources a package can be published to (everything but the bundled catalogue). */
export function publishableSources(sources: RegistrySource[]): RegistrySource[] {
  return sources.filter((s) => s.kind !== "Official" && s.enabled);
}

/** Guesses the package kind from the file name. */
export function guessKind(path: string): PackageKind | null {
  const p = path.trim().toLowerCase();
  if (p.endsWith(".json")) return "AgentTemplate";
  if (p.endsWith(".orkp")) return p.includes("widget") ? "Widget" : "Plugin";
  return null;
}

export function PublishTab({
  version,
  onChanged,
}: {
  version: number;
  onChanged: () => void;
}) {
  const { t } = useT("marketplace");
  const [sources, setSources] = useState<RegistrySource[]>([]);
  const [local, setLocal] = useState<PublisherKey | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [kind, setKind] = useState<PackageKind>("Plugin");
  const [sourceId, setSourceId] = useState("");
  const [publisher, setPublisher] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<MarketEntry | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([marketplaceApi.sources(), marketplaceApi.keys()])
      .then(([s, k]) => {
        if (cancelled) return;
        setSources(s);
        setLocal(k.localPublisher ?? null);
        setLoadError(null);
      })
      .catch((e: unknown) => !cancelled && setLoadError(describeError(e)));
    return () => {
      cancelled = true;
    };
  }, [version]);

  const targets = useMemo(() => publishableSources(sources), [sources]);
  useEffect(() => {
    if (!targets.some((s) => s.id === sourceId)) {
      setSourceId(targets[0]?.id ?? "");
    }
  }, [targets, sourceId]);

  const onPath = (p: string) => {
    setPath(p);
    const k = guessKind(p);
    if (k) setKind(k);
  };

  const publish = async () => {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const entry = await marketplaceApi.publish({
        path: path.trim(),
        kind,
        sourceId,
        publisher: local ? undefined : publisher.trim() || undefined,
      });
      setResult(entry);
      onChanged();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  const needsPublisher = !local && !publisher.trim();
  return (
    <div className="mp-publish">
      {loadError && (
        <p className="mp-error" role="alert">
          {loadError}
        </p>
      )}
      <section className="mp-local" aria-label={t("keys.local")}>
        <h4>
          <KeyRound size={14} /> {t("keys.local")}
        </h4>
        {local ? (
          <>
            <p>
              {t("keys.localPublisher", {
                publisher: local.publisher,
                keyId: local.keyId,
              })}
            </p>
            <div
              className="mp-fingerprint mp-fingerprint--small"
              aria-label={t("keys.fingerprint")}
            >
              {fingerprintRows(local.fingerprint).map((r) => (
                <code key={r}>{r}</code>
              ))}
            </div>
          </>
        ) : (
          <p className="mp-muted">{t("publish.noKey")}</p>
        )}
      </section>
      <form
        aria-label={t("publish.title")}
        onSubmit={(e) => {
          e.preventDefault();
          void publish();
        }}
      >
        <h4>{t("publish.title")}</h4>
        <p className="mp-muted">{t("publish.hint")}</p>
        <PathInput
          kind="file"
          label={t("publish.file")}
          placeholder={t("publish.filePlaceholder")}
          dialogTitle={t("publish.pick")}
          value={path}
          onPathChange={onPath}
        />
        <label className="mp-publish__field">
          <span>{t("publish.kind")}</span>
          <select
            className="mp-select"
            value={kind}
            onChange={(e) => setKind(e.target.value as PackageKind)}
          >
            {PACKAGE_KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`kind.${k}`)}
              </option>
            ))}
          </select>
        </label>
        <label className="mp-publish__field">
          <span>{t("publish.target")}</span>
          <select
            className="mp-select"
            value={sourceId}
            disabled={targets.length === 0}
            onChange={(e) => setSourceId(e.target.value)}
          >
            {targets.map((s) => (
              <option key={s.id} value={s.id}>
                {`${s.name} · ${t(`sourceKind.${s.kind}`)}`}
              </option>
            ))}
          </select>
        </label>
        {targets.length === 0 && (
          <p className="mp-muted">{t("publish.noTargets")}</p>
        )}
        {!local && (
          <Input
            label={t("publish.publisher")}
            hint={t("publish.publisherHint")}
            value={publisher}
            onChange={(e) => setPublisher(e.target.value)}
          />
        )}
        <div>
          <Button
            type="submit"
            variant="primary"
            leftIcon={<Upload size={14} />}
            loading={busy}
            disabled={!path.trim() || !sourceId || needsPublisher}
          >
            {t("publish.submit")}
          </Button>
        </div>
      </form>
      {error && (
        <p className="mp-error" role="alert">
          {t("publish.failed", { error })}
        </p>
      )}
      {result && (
        <div className="mp-publish__result" role="status">
          <p>
            <strong>
              {t("publish.done", {
                name: result.name,
                version: result.version,
              })}
            </strong>{" "}
            <Badge tone="violet">{t(`kind.${result.kind}`)}</Badge>
          </p>
          <p className="mp-muted mp-mono">
            {t("publish.resultMeta", {
              id: result.id,
              source: result.sourceId,
              keyId: result.keyId,
            })}
          </p>
          <p className="mp-muted mp-mono">
            {t("publish.sha", { sha: result.sha256 })}
          </p>
        </div>
      )}
    </div>
  );
}
