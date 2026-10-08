/**
 * Extensions panel (ADR-0025): VS Code extensions from Open VSX or a local
 * `.vsix`, applied to Orkestr's own editor as data (themes, icon themes,
 * grammars, languages, snippets, language servers). Shows publisher,
 * verified status and license, and what of each extension works in the
 * Orkestr editor versus what needs the full VS Code.
 */
import "./extensions.css";

import type { ContributionSummary, InstalledExtension, MarketplaceDetails, MarketplaceExtension } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { BadgeCheck, CircleAlert, Code2, Download, FileArchive, Package, Puzzle, RefreshCw, Search, Star, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { PathInput } from "../../components/PathInput";
import { getClient, maybeClient } from "../../core/client";
import { SafeMarkdown } from "../../features/extensions/markdown";
import { supportedItems, supportLevel, unsupportedKinds } from "../../features/extensions/model";
import { ensureExtensionsLoaded, useExtensionsStore } from "../../features/extensions/store";
import { ExtensionLauncher } from "../../features/vscode/ExtensionLauncher";
import { openProjectInVsCode, openVsCodePanel } from "../../features/vscode/quickAccess";
import { useVsCodeStore } from "../../features/vscode/store";
import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";

type View = "marketplace" | "installed" | "vscode";

/** What the details pane shows. */
type Selection = { kind: "market"; namespace: string; name: string } | { kind: "installed"; id: string };

const INSTALL_TIMEOUT = 10 * 60_000;
const iconCache = new Map<string, Promise<string | null>>();

function loadIcon(key: string, load: () => Promise<string | null>): Promise<string | null> {
  let p = iconCache.get(key);
  if (!p) {
    p = load().catch(() => null);
    iconCache.set(key, p);
  }
  return p;
}

function ExtIcon({ url, installed }: { url?: string | null; installed?: InstalledExtension }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const client = maybeClient();
    if (!client) return;
    let p: Promise<string | null> | null = null;
    if (url) p = loadIcon(url, () => client.call("extensions.icon", { url }).then((r) => r.dataUrl));
    else if (installed?.icon && installed.enabled) {
      const iconPath = installed.icon;
      p = loadIcon(`${installed.id}@${installed.version}`, () =>
        client
          .call("extensions.readAsset", { extensionId: installed.id, path: iconPath })
          .then((a) => (a.base64 ? `data:${a.mime};base64,${a.base64}` : null)),
      );
    }
    void p?.then((s) => alive && setSrc(s));
    return () => {
      alive = false;
    };
  }, [url, installed]);
  return src ? (
    <img className="ext-icon" src={src} alt="" aria-hidden="true" draggable={false} />
  ) : (
    <span className="ext-icon ext-icon--placeholder" aria-hidden="true">
      <Puzzle size={20} />
    </span>
  );
}

function SupportBadge({ c }: { c: ContributionSummary }) {
  const { t } = useT("extensions");
  const level = supportLevel(c);
  return (
    <Badge tone={level === "full" ? "success" : level === "partial" ? "warning" : "neutral"}>
      {t(`support.${level}`)}
    </Badge>
  );
}

function Compatibility({ c, extensionId }: { c: ContributionSummary; extensionId: string | null }) {
  const { t } = useT("extensions");
  const supported = supportedItems(c);
  const missing = unsupportedKinds(c);
  return (
    <section className="ext-compat" aria-label={t("compat.title")}>
      <h3>{t("compat.works")}</h3>
      {supported.length === 0 ? (
        <p className="ext-muted">{t("compat.nothing")}</p>
      ) : (
        <ul className="ext-compat__list">
          {supported.map((s) => (
            <li key={s.key}>
              <BadgeCheck size={14} className="ext-ok" aria-hidden="true" />
              <span>
                {t(`compat.kind.${s.key}`, { count: s.count })}
                {s.detail && <span className="ext-muted"> — {s.detail}</span>}
              </span>
            </li>
          ))}
        </ul>
      )}
      {(missing.length > 0 || c.hasCode) && (
        <>
          <h3>{t("compat.needsVsCode")}</h3>
          <ul className="ext-compat__list">
            {missing.map((k) => (
              <li key={k}>
                <CircleAlert size={14} className="ext-warn" aria-hidden="true" />
                <code>{k}</code>
              </li>
            ))}
            {c.hasCode && (
              <li>
                <CircleAlert size={14} className="ext-warn" aria-hidden="true" />
                <span>{t("compat.code")}</span>
              </li>
            )}
          </ul>
          {extensionId && <InstallInVsCode extensionId={extensionId} />}
        </>
      )}
    </section>
  );
}

/**
 * Extensions that need the full VS Code (commands, webviews, extension
 * code — e.g. Claude Code) can be installed into Orkestr's embedded VS Code
 * (VSCodium, `vscode.installExtension`) from here.
 */
function InstallInVsCode({ extensionId }: { extensionId: string }) {
  const { t } = useT("extensions");
  const status = useVsCodeStore((s) => s.status);
  const vsExtensions = useVsCodeStore((s) => s.extensions);
  const installing = useVsCodeStore((s) => s.installingExtension);
  const vsError = useVsCodeStore((s) => s.extensionsError);
  const [tried, setTried] = useState(false);
  const id = extensionId.toLowerCase();

  useEffect(() => {
    const store = useVsCodeStore.getState();
    if (!store.status) void store.load();
  }, []);
  const vsInstalled = status?.installed != null;
  useEffect(() => {
    if (vsInstalled && useVsCodeStore.getState().extensions === null) void useVsCodeStore.getState().loadExtensions();
  }, [vsInstalled]);

  const present = vsExtensions?.some((x) => x.id === id) ?? false;
  const busy = installing !== null && installing.toLowerCase() === id;

  if (!status) return null;
  return (
    <div className="ext-vscode">
      {!vsInstalled ? (
        <>
          <p className="ext-muted">{t("vscode.missing")}</p>
          <Button size="sm" variant="secondary" leftIcon={<Code2 size={13} />} onClick={openVsCodePanel}>
            {t("vscode.openPanel")}
          </Button>
        </>
      ) : present ? (
        <>
          <p className="ext-muted">
            <BadgeCheck size={14} className="ext-ok" aria-hidden="true" /> {t("vscode.installed")}
          </p>
          <Button
            size="sm"
            variant="secondary"
            leftIcon={<Code2 size={13} />}
            onClick={() => void openProjectInVsCode(useAppStore.getState().activeProjectId)}
          >
            {t("vscode.open")}
          </Button>
          <ExtensionLauncher only={id} showHeader={false} />
        </>
      ) : (
        <>
          <p className="ext-muted">{t("vscode.hint")}</p>
          <Button
            size="sm"
            variant="primary"
            leftIcon={<Code2 size={13} />}
            loading={busy}
            disabled={installing !== null}
            onClick={() => {
              setTried(true);
              void useVsCodeStore.getState().installExtension(id);
            }}
          >
            {busy ? t("vscode.installing") : t("vscode.install")}
          </Button>
          {tried && !busy && vsError && <p className="ext-error">{vsError}</p>}
        </>
      )}
    </div>
  );
}

function Publisher({ name, verified }: { name: string; verified: boolean | null }) {
  const { t } = useT("extensions");
  return (
    <span className="ext-publisher">
      {name}
      {verified === true && (
        <span className="ext-verified" title={t("verified.tooltip")}>
          <BadgeCheck size={13} aria-hidden="true" /> {t("verified.label")}
        </span>
      )}
      {verified === false && <span className="ext-unverified" title={t("unverified.tooltip")}>{t("unverified.label")}</span>}
    </span>
  );
}

function ProgressLine({ keyName }: { keyName: string }) {
  const { t } = useT("extensions");
  const fmt = useFormat();
  const p = useExtensionsStore((s) => s.progress[keyName]);
  if (!p) return null;
  if (p.phase === "failed") return <span className="ext-error">{p.error ?? t("install.failed")}</span>;
  const pct = p.total ? Math.min(1, p.received / p.total) : null;
  return (
    <span className="ext-progress" role="status">
      <Spinner size={12} /> {t(`install.phase.${p.phase}`)}
      {pct !== null && p.phase === "downloading" && ` ${fmt.percent(pct)}`}
      {p.received > 0 && p.phase === "downloading" && <span className="ext-muted"> · {fmt.bytes(p.received)}</span>}
    </span>
  );
}

function useInstalledMap(): Map<string, InstalledExtension> {
  const installed = useExtensionsStore((s) => s.installed);
  return useMemo(() => new Map(installed.map((e) => [e.id, e])), [installed]);
}

function InstalledActions({ ext, onError }: { ext: InstalledExtension; onError: (e: string | null) => void }) {
  const { t } = useT("extensions");
  const [busy, setBusy] = useState<"toggle" | "remove" | null>(null);
  const run = async (what: "toggle" | "remove") => {
    setBusy(what);
    onError(null);
    try {
      if (what === "toggle") await getClient().call("extensions.setEnabled", { id: ext.id, enabled: !ext.enabled });
      else await getClient().call("extensions.uninstall", { id: ext.id });
      await useExtensionsStore.getState().load();
    } catch (e) {
      onError(describeError(e));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="ext-actions">
      <Button size="sm" variant="secondary" loading={busy === "toggle"} disabled={busy !== null} onClick={() => void run("toggle")}>
        {ext.enabled ? t("actions.disable") : t("actions.enable")}
      </Button>
      <Button size="sm" variant="danger" loading={busy === "remove"} disabled={busy !== null} onClick={() => void run("remove")}>
        {t("actions.uninstall")}
      </Button>
    </div>
  );
}

function InstallButton({ namespace, name, installedVersion, latest }: { namespace: string; name: string; installedVersion: string | null; latest: string }) {
  const { t } = useT("extensions");
  const key = `${namespace}.${name}`.toLowerCase();
  const progress = useExtensionsStore((s) => s.progress[key]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await getClient().call("extensions.install", { namespace, name }, { timeoutMs: INSTALL_TIMEOUT });
      await useExtensionsStore.getState().load();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };
  const update = installedVersion !== null && installedVersion !== latest;
  return (
    <span className="ext-install">
      {installedVersion !== null && !update ? (
        <Badge tone="success">{t("installed.badge")}</Badge>
      ) : (
        <Button
          size="sm"
          variant="primary"
          leftIcon={<Download size={13} />}
          loading={busy || (progress !== undefined && progress.phase !== "failed")}
          onClick={(e) => {
            e.stopPropagation();
            void install();
          }}
        >
          {update ? t("actions.update", { version: latest }) : t("actions.install")}
        </Button>
      )}
      <ProgressLine keyName={key} />
      {error && <span className="ext-error">{error}</span>}
    </span>
  );
}

function MarketItem({ ext, selected, onSelect }: { ext: MarketplaceExtension; selected: boolean; onSelect: () => void }) {
  const { t } = useT("extensions");
  const fmt = useFormat();
  return (
    <li
      className={clsx("ext-item", selected && "ext-item--selected")}
      aria-selected={selected}
      role="option"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && onSelect()}
    >
      <ExtIcon url={ext.iconUrl} />
      <div className="ext-item__body">
        <div className="ext-item__title">
          <span className="ext-item__name" title={ext.id}>
            {ext.displayName}
          </span>
          <span className="ext-muted">{ext.version}</span>
        </div>
        <Publisher name={ext.publisherDisplayName ?? ext.namespace} verified={ext.verified} />
        {ext.description && <p className="ext-item__desc">{ext.description}</p>}
        <div className="ext-item__meta">
          <span title={t("meta.downloads")}>
            <Download size={12} aria-hidden="true" /> {fmt.number(ext.downloads, { notation: "compact", maximumFractionDigits: 1 })}
          </span>
          {ext.rating !== null && (
            <span title={t("meta.rating")}>
              <Star size={12} aria-hidden="true" /> {fmt.number(ext.rating, { maximumFractionDigits: 1 })}
            </span>
          )}
          {ext.license && <span title={t("meta.license")}>{ext.license}</span>}
        </div>
      </div>
      <InstallButton namespace={ext.namespace} name={ext.name} installedVersion={ext.installedVersion} latest={ext.version} />
    </li>
  );
}

function InstalledItem({ ext, selected, onSelect }: { ext: InstalledExtension; selected: boolean; onSelect: () => void }) {
  const { t } = useT("extensions");
  return (
    <li
      className={clsx("ext-item", selected && "ext-item--selected", !ext.enabled && "ext-item--disabled")}
      aria-selected={selected}
      role="option"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && onSelect()}
    >
      <ExtIcon installed={ext} />
      <div className="ext-item__body">
        <div className="ext-item__title">
          <span className="ext-item__name" title={ext.id}>
            {ext.displayName}
          </span>
          <span className="ext-muted">{ext.version}</span>
          {!ext.enabled && <Badge tone="neutral">{t("installed.disabled")}</Badge>}
        </div>
        <Publisher name={ext.publisher} verified={ext.verified} />
        {ext.description && <p className="ext-item__desc">{ext.description}</p>}
        <div className="ext-item__meta">
          <SupportBadge c={ext.contributions} />
          <span>{ext.source === "openVsx" ? t("source.openVsx") : t("source.vsix")}</span>
          {ext.license && <span title={t("meta.license")}>{ext.license}</span>}
        </div>
      </div>
    </li>
  );
}

function Details({ selection, onClose }: { selection: Selection; onClose: () => void }) {
  const { t } = useT("extensions");
  const fmt = useFormat();
  const installedMap = useInstalledMap();
  const [details, setDetails] = useState<MarketplaceDetails | null>(null);
  const [readme, setReadme] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setDetails(null);
    setReadme(null);
    setError(null);
    setLoading(true);
    const client = getClient();
    const load =
      selection.kind === "market"
        ? client.call("extensions.details", { namespace: selection.namespace, name: selection.name }, { timeoutMs: 60_000 }).then((d) => {
            if (!alive) return;
            setDetails(d);
            setReadme(d.readme);
          })
        : client.call("extensions.readme", { id: selection.id }).then((r) => alive && setReadme(r.markdown));
    load
      .catch((e: unknown) => alive && setError(describeError(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [selection]);

  const installed = selection.kind === "installed" ? installedMap.get(selection.id) : details ? installedMap.get(details.extension.id) : undefined;
  const header = details?.extension;
  const contributions = installed?.contributions ?? details?.contributions ?? null;

  return (
    <aside className="ext-details" aria-label={t("details.label")}>
      <div className="ext-details__head">
        {header ? <ExtIcon url={header.iconUrl} /> : installed ? <ExtIcon installed={installed} /> : null}
        <div className="ext-details__title">
          <h2>{header?.displayName ?? installed?.displayName ?? ""}</h2>
          {(header || installed) && (
            <Publisher
              name={header?.publisherDisplayName ?? header?.namespace ?? installed?.publisher ?? ""}
              verified={header ? header.verified : (installed?.verified ?? null)}
            />
          )}
        </div>
        <IconButton label={t("details.close")} icon={<X size={14} />} size="sm" onClick={onClose} />
      </div>
      {loading && (
        <div className="ext-muted ext-loading">
          <Spinner size={14} /> {t("details.loading")}
        </div>
      )}
      {error && <div className="ext-error">{error}</div>}
      <dl className="ext-facts">
        <dt>{t("facts.id")}</dt>
        <dd>
          <code>{header?.id ?? installed?.id}</code>
        </dd>
        <dt>{t("facts.version")}</dt>
        <dd>
          {installed ? installed.version : header?.version}
          {details?.preRelease && <Badge tone="warning">{t("facts.preRelease")}</Badge>}
        </dd>
        <dt>{t("facts.license")}</dt>
        <dd>{header?.license ?? installed?.license ?? t("facts.unknown")}</dd>
        {header && (
          <>
            <dt>{t("meta.downloads")}</dt>
            <dd>{fmt.integer(header.downloads)}</dd>
          </>
        )}
        {(details?.engine ?? installed?.engine) && (
          <>
            <dt>{t("facts.engine")}</dt>
            <dd>
              <code>{details?.engine ?? installed?.engine}</code>
            </dd>
          </>
        )}
        {details && (
          <>
            <dt>{t("facts.platform")}</dt>
            <dd>
              {!details.installable
                ? t("facts.noBuild", { target: details.installTarget })
                : details.targetPlatforms.includes(details.installTarget)
                  ? details.installTarget
                  : t("facts.universal")}
            </dd>
          </>
        )}
        {details?.repository && (
          <>
            <dt>{t("facts.repository")}</dt>
            <dd className="ext-url">{details.repository}</dd>
          </>
        )}
        {installed && (
          <>
            <dt>{t("facts.source")}</dt>
            <dd>{installed.source === "openVsx" ? t("source.openVsx") : t("source.vsix")}</dd>
            <dt>{t("facts.sha256")}</dt>
            <dd className="ext-url">
              <code>{installed.sha256}</code>
            </dd>
          </>
        )}
      </dl>
      <div className="ext-details__actions">
        {installed && <InstalledActions ext={installed} onError={setActionError} />}
        {header && details?.installable && (
          <InstallButton namespace={header.namespace} name={header.name} installedVersion={installed?.version ?? null} latest={header.version} />
        )}
        {actionError && <span className="ext-error">{actionError}</span>}
      </div>
      {contributions && <Compatibility c={contributions} extensionId={header?.id ?? installed?.id ?? null} />}
      {readme && (
        <section className="ext-readme" aria-label={t("details.readme")}>
          <SafeMarkdown source={readme} />
        </section>
      )}
    </aside>
  );
}

function VsixForm({ onDone }: { onDone: () => void }) {
  const { t } = useT("extensions");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    if (!path.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await getClient().call("extensions.installVsix", { path: path.trim() }, { timeoutMs: INSTALL_TIMEOUT });
      await useExtensionsStore.getState().load();
      onDone();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="ext-vsix"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label className="ext-vsix__label" htmlFor="ext-vsix-path">
        {t("vsix.label")}
      </label>
      <div className="ext-vsix__row">
        <PathInput
          id="ext-vsix-path"
          kind="file"
          dialogTitle={t("vsix.dialogTitle")}
          value={path}
          onPathChange={setPath}
          placeholder={t("vsix.placeholder")}
          autoFocus
        />
        <Button type="submit" variant="primary" size="sm" loading={busy} disabled={!path.trim()}>
          {t("vsix.install")}
        </Button>
        <Button variant="ghost" size="sm" onClick={onDone}>
          {t("vsix.cancel")}
        </Button>
      </div>
      <p className="ext-muted">{t("vsix.hint")}</p>
      {error && <p className="ext-error">{error}</p>}
    </form>
  );
}

/** Extensions panel. */
export default function ExtensionsPanel(_props: PanelProps) {
  const { t } = useT("extensions");
  const fmt = useFormat();
  const connected = useAppStore((s) => s.connection.state === "open");
  const installed = useExtensionsStore((s) => s.installed);
  const [view, setView] = useState<View>("marketplace");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MarketplaceExtension[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [vsixOpen, setVsixOpen] = useState(false);
  const seq = useRef(0);

  useEffect(() => {
    if (connected) ensureExtensionsLoaded();
  }, [connected]);

  const search = useCallback(
    async (q: string, offset: number) => {
      const my = ++seq.current;
      setLoading(true);
      setError(null);
      try {
        const res = await getClient().call("extensions.search", { query: q, offset, size: 30, sort: q.trim() ? "relevance" : "downloads" }, { timeoutMs: 60_000 });
        if (my !== seq.current) return;
        setTotal(Number(res.total));
        setResults((prev) => (offset === 0 ? res.items : [...prev, ...res.items]));
      } catch (e) {
        if (my === seq.current) setError(describeError(e));
      } finally {
        if (my === seq.current) setLoading(false);
      }
    },
    [],
  );

  // Debounced search (popular extensions when the box is empty).
  useEffect(() => {
    if (!connected || view !== "marketplace") return;
    const id = setTimeout(() => void search(query, 0), query ? 350 : 0);
    return () => clearTimeout(id);
  }, [query, connected, view, search]);

  // Refresh "installed" markers after installs.
  const installedVersions = useMemo(() => new Map(installed.map((e) => [e.id, e.version])), [installed]);
  const shown = useMemo(
    () => results.map((r) => ({ ...r, installedVersion: installedVersions.get(r.id) ?? null })),
    [results, installedVersions],
  );

  const tabs = [
    { id: "marketplace" as const, label: t("tabs.marketplace") },
    { id: "installed" as const, label: t("tabs.installed", { n: installed.length }) },
    { id: "vscode" as const, label: t("tabs.vscode") },
  ];

  return (
    <div className="ext-panel">
      <div className="ext-toolbar">
        <Tabs items={tabs} value={view} onChange={setView} aria-label={t("tabs.label")} />
        <div className="ext-toolbar__spacer" />
        <Button size="sm" variant="secondary" leftIcon={<FileArchive size={13} />} onClick={() => setVsixOpen((v) => !v)}>
          {t("vsix.button")}
        </Button>
      </div>
      {vsixOpen && <VsixForm onDone={() => setVsixOpen(false)} />}
      {view === "vscode" ? (
        <div className="ext-launcher">
          <ExtensionLauncher />
        </div>
      ) : (
      <div className="ext-body">
        <div className="ext-listpane">
          {view === "marketplace" ? (
            <>
              <div className="ext-search">
                <Input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder={t("search.placeholder")}
                  aria-label={t("search.label")}
                  leftIcon={<Search size={13} />}
                />
                <IconButton label={t("search.refresh")} icon={<RefreshCw size={13} />} size="sm" onClick={() => void search(query, 0)} />
              </div>
              <p className="ext-source-note">{t("search.source")}</p>
              {error && <div className="ext-error">{error}</div>}
              {!error && !loading && shown.length === 0 ? (
                <EmptyState icon={<Package size={24} />} title={t("search.empty")} description={t("search.emptyHint")} />
              ) : (
                <ul className="ext-list" role="listbox" aria-label={t("search.results")}>
                  {shown.map((ext) => (
                    <MarketItem
                      key={ext.id}
                      ext={ext}
                      selected={selection?.kind === "market" && selection.namespace === ext.namespace && selection.name === ext.name}
                      onSelect={() => setSelection({ kind: "market", namespace: ext.namespace, name: ext.name })}
                    />
                  ))}
                </ul>
              )}
              <div className="ext-more">
                {loading && <Spinner size={14} label={t("search.loading")} />}
                {!loading && results.length < total && (
                  <Button size="sm" variant="ghost" onClick={() => void search(query, results.length)}>
                    {t("search.more", { shown: fmt.integer(results.length), total: fmt.integer(total) })}
                  </Button>
                )}
              </div>
            </>
          ) : installed.length === 0 ? (
            <EmptyState icon={<Puzzle size={24} />} title={t("installed.empty")} description={t("installed.emptyHint")} />
          ) : (
            <ul className="ext-list" role="listbox" aria-label={t("installed.label")}>
              {installed.map((ext) => (
                <InstalledItem
                  key={ext.id}
                  ext={ext}
                  selected={selection?.kind === "installed" && selection.id === ext.id}
                  onSelect={() => setSelection({ kind: "installed", id: ext.id })}
                />
              ))}
            </ul>
          )}
        </div>
        {selection && <Details selection={selection} onClose={() => setSelection(null)} />}
      </div>
      )}
    </div>
  );
}
