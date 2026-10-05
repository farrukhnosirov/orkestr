/**
 * Architecture panel (§21, §68.48): the auto-generated model of the project —
 * modules, services, databases, APIs and dependencies — as an interactive map
 * (or a grouped list), with search / highlight, kind filters, node details
 * (files open in Code), snapshot history, change highlighting against the
 * previous snapshot, manual refresh and SVG export. The core refreshes the
 * map incrementally after file changes (`ArchitectureMapUpdated`).
 */
import type { ArchDiff, ArchMapResult, ArchNodeKind } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, Download, GitCompare, Network, RefreshCw, ScanSearch, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import type { PanelProps } from "../types";
import "./architecture.css";
import { KIND_ICON, KIND_TOKEN, kindClass } from "./kinds";
import { MapCanvas } from "./MapCanvas";
import { diffMarks, exportSvg, highlightSet, matchesQuery, NODE_KINDS, visibleGraph } from "./model";
import { NodeDetail } from "./NodeDetail";

type View = "map" | "list";

const MAX_REMOVED_NAMES = 6;

function cssVar(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function download(name: string, text: string, type: string): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export default function ArchitecturePanel({ projectId }: PanelProps) {
  const { t } = useT(["architecture", "common"]);
  const fmt = useFormat();
  const [snapshotId, setSnapshotId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hidden, setHidden] = useState<Set<ArchNodeKind>>(() => new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<View>("map");
  const [showChanges, setShowChanges] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [notice, setNotice] = useState<{ tone: "info" | "danger"; text: string } | null>(null);

  useEffect(() => {
    setSnapshotId(null);
    setSelected(null);
    setNotice(null);
  }, [projectId]);

  const topics = projectId ? [topic.project(projectId)] : [];
  const mapQuery = useRpcQuery<ArchMapResult>(
    `arch|${projectId ?? ""}|${snapshotId ?? "latest"}`,
    (c) => c.call("architecture.map", snapshotId ? { projectId: projectId ?? "", snapshotId } : { projectId: projectId ?? "" }),
    { enabled: projectId !== null, topics, reloadOn: ["ArchitectureMapUpdated"] },
  );
  const map = mapQuery.data?.map ?? null;
  const snapshots = mapQuery.data?.snapshots ?? [];

  const diffQuery = useRpcQuery<ArchDiff>(
    `arch-diff|${projectId ?? ""}|${map?.snapshotId ?? ""}`,
    async (c) => (await c.call("architecture.diff", { projectId: projectId ?? "", to: map?.snapshotId ?? "" })).diff,
    { enabled: projectId !== null && showChanges && map !== null },
  );
  const diff = showChanges ? diffQuery.data : null;
  const marks = useMemo(() => diffMarks(diff), [diff]);

  const graph = useMemo(() => (map ? visibleGraph(map, hidden) : { nodes: [], edges: [] }), [map, hidden]);
  const highlight = useMemo(() => highlightSet(graph.nodes, graph.edges, query, selected), [graph, query, selected]);
  const selectedNode = map?.nodes.find((n) => n.id === selected) ?? null;
  const counts = useMemo(() => {
    const c: Record<ArchNodeKind, number> = { Service: 0, Module: 0, Api: 0, Database: 0, External: 0 };
    for (const n of map?.nodes ?? []) c[n.kind] += 1;
    return c;
  }, [map]);

  const toggleKind = (k: ArchNodeKind) =>
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });

  const refresh = useCallback(
    async (full: boolean) => {
      if (!projectId) return;
      setRefreshing(true);
      setNotice(null);
      try {
        const res = await requireClient().call("architecture.refresh", full ? { projectId, full: true } : { projectId });
        setSnapshotId(null);
        setNotice({ tone: "info", text: res.changed ? t("refreshed") : t("unchanged") });
        mapQuery.reload();
        if (showChanges) diffQuery.reload();
      } catch (e) {
        setNotice({ tone: "danger", text: errorMessage(e) });
      } finally {
        setRefreshing(false);
      }
    },
    [projectId, t, mapQuery, diffQuery, showChanges],
  );

  const onExport = () => {
    if (!map) return;
    const svg = exportSvg(
      graph.nodes,
      graph.edges,
      {
        background: cssVar("--ok-color-bg", "#0b0d12"),
        text: cssVar("--ok-color-text", "#e8eaf0"),
        muted: cssVar("--ok-color-text-muted", "#9aa0ad"),
        edge: cssVar("--ok-color-border-strong", "#4a5060"),
        kind: {
          Service: cssVar(KIND_TOKEN.Service, "#3ec5ff"),
          Module: cssVar(KIND_TOKEN.Module, "#9b7bff"),
          Api: cssVar(KIND_TOKEN.Api, "#5aa9ff"),
          Database: cssVar(KIND_TOKEN.Database, "#ffb84d"),
          External: cssVar(KIND_TOKEN.External, "#9aa0ad"),
        },
      },
      (k) => t(`kind.${k}`),
    );
    download(`architecture-${map.generatedAt.slice(0, 10)}.svg`, svg, "image/svg+xml");
  };

  if (!projectId) {
    return (
      <div className="arch-panel arch-panel--center">
        <EmptyState icon={<Network size={26} strokeWidth={1.6} />} title={t("noProject.title")} description={t("noProject.description")} />
      </div>
    );
  }

  if (!map) {
    return (
      <div className="arch-panel arch-panel--center">
        {mapQuery.error ? (
          <EmptyState
            icon={<AlertTriangle size={26} strokeWidth={1.6} />}
            title={t("error.title")}
            description={mapQuery.error}
            action={
              <Button variant="secondary" onClick={mapQuery.reload} leftIcon={<RefreshCw size={14} />}>
                {t("common:actions.retry")}
              </Button>
            }
          />
        ) : (
          <div className="arch-loading" role="status">
            <Spinner size={18} /> {t("loading")}
          </div>
        )}
      </div>
    );
  }

  const removedNames = (diff?.removedNodes ?? []).map((n) => n.name);
  const fromSnapshot = diff?.fromSnapshotId ? snapshots.find((s) => s.snapshotId === diff.fromSnapshotId) : undefined;

  return (
    <div className="arch-panel">
      <div className="arch-toolbar" role="toolbar" aria-label={t("toolbar.label")}>
        <Input
          className="arch-search"
          placeholder={t("toolbar.search")}
          aria-label={t("toolbar.searchLabel")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          leftIcon={<Search size={14} />}
        />
        <Tabs
          aria-label={t("toolbar.view")}
          items={[
            { id: "map", label: t("view.map") },
            { id: "list", label: t("view.list") },
          ]}
          value={view}
          onChange={setView}
        />
        <select
          className="arch-select"
          aria-label={t("toolbar.snapshot")}
          value={snapshotId ?? ""}
          onChange={(e) => setSnapshotId(e.target.value || null)}
        >
          <option value="">{t("toolbar.latest")}</option>
          {snapshots.slice(1).map((s) => (
            <option key={s.snapshotId} value={s.snapshotId}>
              {fmt.date(s.generatedAt, "dateTime")}
            </option>
          ))}
        </select>
        <div className="arch-toolbar__spacer" />
        <IconButton
          label={t("toolbar.changes")}
          icon={<GitCompare size={15} />}
          active={showChanges}
          aria-pressed={showChanges}
          onClick={() => setShowChanges((v) => !v)}
          tooltip="bottom"
        />
        <IconButton label={t("toolbar.exportSvg")} icon={<Download size={15} />} onClick={onExport} tooltip="bottom" />
        <IconButton
          label={t("toolbar.refresh")}
          icon={refreshing ? <Spinner size={14} /> : <RefreshCw size={15} />}
          disabled={refreshing}
          onClick={() => void refresh(false)}
          tooltip="bottom"
        />
        <IconButton
          label={t("toolbar.fullRefresh")}
          icon={<ScanSearch size={15} />}
          disabled={refreshing}
          onClick={() => void refresh(true)}
          tooltip="bottom"
        />
      </div>

      <div className="arch-legend" role="group" aria-label={t("legend.label")}>
        {NODE_KINDS.map((k) => {
          const Icon = KIND_ICON[k];
          const on = !hidden.has(k);
          return (
            <button
              key={k}
              type="button"
              className={clsx("arch-chip", kindClass(k), !on && "arch-chip--off")}
              aria-pressed={on}
              onClick={() => toggleKind(k)}
            >
              <Icon size={12} /> {t("legend.toggle", { kind: t(`group.${k}`), count: counts[k] })}
            </button>
          );
        })}
        <span className="arch-legend__note">{t("legend.dashed")}</span>
      </div>

      <div className="arch-status" aria-live="polite">
        <span>{t("stats.scanned", { time: fmt.relative(map.generatedAt) })}</span>
        <span>{t("stats.files", { count: map.stats.filesScanned })}</span>
        <span>{t("stats.routes", { count: map.stats.routes })}</span>
        <span>{t("stats.edges", { count: map.stats.edges })}</span>
        <span>{t("stats.scanTime", { duration: fmt.duration(Number(map.stats.scanMs)) })}</span>
        {map.warnings.length > 0 && (
          <Badge tone="warning" title={map.warnings.join("\n")}>
            {t("warnings", { count: map.warnings.length })}
          </Badge>
        )}
        {notice && <span className={clsx("arch-status__notice", notice.tone === "danger" && "arch-status__notice--danger")}>{notice.text}</span>}
      </div>
      {map.truncated && (
        <div className="arch-banner arch-banner--warning" role="note">
          <AlertTriangle size={14} /> {t("truncated")}
        </div>
      )}
      {showChanges && diff && (
        <div className="arch-banner" role="note">
          <GitCompare size={14} />{" "}
          {!diff.fromSnapshotId
            ? t("diff.first")
            : diff.addedNodes.length + diff.changedNodes.length + diff.removedNodes.length + diff.addedEdges.length + diff.removedEdges.length === 0
              ? t("diff.none")
              : t("diff.summary", {
                  time: fromSnapshot ? fmt.date(fromSnapshot.generatedAt, "dateTime") : "",
                  added: diff.addedNodes.length,
                  changed: diff.changedNodes.length,
                  removed: diff.removedNodes.length,
                })}
          {diff.fromSnapshotId && removedNames.length > 0 && (
            <span className="arch-banner__removed">
              {" · "}
              {t("diff.removed", {
                names:
                  fmt.list(removedNames.slice(0, MAX_REMOVED_NAMES)) +
                  (removedNames.length > MAX_REMOVED_NAMES ? ` ${t("detail.more", { count: removedNames.length - MAX_REMOVED_NAMES })}` : ""),
              })}
            </span>
          )}
        </div>
      )}

      <div className="arch-body">
        {map.nodes.length === 0 ? (
          <div className="arch-panel--center arch-body__main">
            <EmptyState icon={<Network size={26} strokeWidth={1.6} />} title={t("empty.title")} description={t("empty.description")} />
          </div>
        ) : view === "map" ? (
          <MapCanvas
            nodes={graph.nodes}
            edges={graph.edges}
            highlight={highlight}
            marks={showChanges && diff?.fromSnapshotId ? marks : new Map()}
            selected={selected}
            onSelect={setSelected}
          />
        ) : (
          <div className="arch-list arch-body__main" role="region" aria-label={t("list.label")}>
            {NODE_KINDS.filter((k) => !hidden.has(k)).map((k) => {
              const items = graph.nodes.filter((n) => n.kind === k && matchesQuery(n, query));
              if (items.length === 0) return null;
              const Icon = KIND_ICON[k];
              return (
                <section key={k} className={clsx("arch-list__group", kindClass(k))}>
                  <h4>
                    <Icon size={13} /> {t(`group.${k}`)} <span className="arch-list__count">{items.length}</span>
                  </h4>
                  <ul>
                    {items.map((n) => {
                      const mark = showChanges && diff?.fromSnapshotId ? (marks.get(n.id) ?? null) : null;
                      return (
                        <li key={n.id}>
                          <button
                            type="button"
                            className={clsx("arch-list__item", selected === n.id && "arch-list__item--selected")}
                            aria-current={selected === n.id || undefined}
                            onClick={() => setSelected(n.id)}
                          >
                            <span className="arch-list__name">{n.name}</span>
                            <span className="arch-list__meta arch-mono">{n.path ?? n.ecosystem}</span>
                            {mark && <Badge tone={mark === "added" ? "success" : "warning"}>{t(`diff.${mark}`)}</Badge>}
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                </section>
              );
            })}
          </div>
        )}
        {selectedNode && (
          <NodeDetail
            node={selectedNode}
            nodes={map.nodes}
            edges={map.edges}
            mark={showChanges && diff?.fromSnapshotId ? (marks.get(selectedNode.id) ?? null) : null}
            onSelect={setSelected}
          />
        )}
      </div>
    </div>
  );
}
