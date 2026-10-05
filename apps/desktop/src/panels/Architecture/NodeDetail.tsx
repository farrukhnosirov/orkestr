/** Side drawer with everything known about one node; files open in the Code panel. */
import type { ArchEdge, ArchNode } from "@orkestr/protocol";
import { Badge, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { ArrowLeft, ArrowRight, FileCode, X } from "lucide-react";

import { openInEditor } from "../../features/files/actions";
import { useT } from "../../i18n";
import { KIND_ICON, kindClass } from "./kinds";
import { type DiffMark, edgesOf, nodeFiles } from "./model";

const MAX_LIST = 40;

export interface NodeDetailProps {
  node: ArchNode;
  nodes: readonly ArchNode[];
  edges: readonly ArchEdge[];
  mark: DiffMark | null;
  onSelect: (id: string | null) => void;
}

export function NodeDetail({ node, nodes, edges, mark, onSelect }: NodeDetailProps) {
  const { t } = useT("architecture");
  const Icon = KIND_ICON[node.kind];
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const { incoming, outgoing } = edgesOf(edges, node.id);
  const files = nodeFiles(node);
  const routeFile = (file: string, line: number) => openInEditor(file, { line });

  const edgeList = (list: ArchEdge[], dir: "in" | "out") =>
    list.map((e) => {
      const other = byId.get(dir === "in" ? e.from : e.to);
      if (!other) return null;
      return (
        <li key={e.id}>
          <button type="button" className="arch-detail__link" onClick={() => onSelect(other.id)}>
            {dir === "in" ? <ArrowLeft size={12} /> : <ArrowRight size={12} />}
            <span className={clsx("arch-detail__dot", kindClass(other.kind))} />
            <span className="arch-detail__name">{other.name}</span>
            <span className="arch-detail__muted">{t(`edge.${e.kind}`)}</span>
          </button>
        </li>
      );
    });

  return (
    <aside className="arch-detail" aria-label={t("detail.label", { name: node.name })}>
      <header className={clsx("arch-detail__head", kindClass(node.kind))}>
        <Icon size={16} />
        <div className="arch-detail__title">
          <div className="arch-detail__kind">
            {t(`kind.${node.kind}`)} · {node.ecosystem}
          </div>
          <h3>{node.name}</h3>
        </div>
        {mark && <Badge tone={mark === "added" ? "success" : "warning"}>{t(`diff.${mark}`)}</Badge>}
        <IconButton label={t("detail.close")} icon={<X size={14} />} size="sm" onClick={() => onSelect(null)} />
      </header>

      <dl className="arch-detail__props">
        {node.path && (
          <>
            <dt>{t("detail.path")}</dt>
            <dd className="arch-mono">{node.path}</dd>
          </>
        )}
        {node.detail && (
          <>
            <dt>{t("detail.info")}</dt>
            <dd>{node.detail}</dd>
          </>
        )}
        {node.fileCount > 0 && (
          <>
            <dt>{t("detail.fileCount")}</dt>
            <dd>{node.fileCount}</dd>
          </>
        )}
        {node.tags.length > 0 && (
          <>
            <dt>{t("detail.tags")}</dt>
            <dd className="arch-detail__tags">
              {node.tags.map((tag) => (
                <Badge key={tag}>{tag}</Badge>
              ))}
            </dd>
          </>
        )}
      </dl>

      <section>
        <h4>{t("detail.files")}</h4>
        {files.length === 0 ? (
          <p className="arch-detail__muted">{t("detail.noFiles")}</p>
        ) : (
          <ul className="arch-detail__list">
            {files.slice(0, MAX_LIST).map((f) => (
              <li key={f}>
                <button
                  type="button"
                  className="arch-detail__link arch-mono"
                  onClick={() => openInEditor(f)}
                  aria-label={t("detail.openFile", { path: f })}
                  title={t("detail.openFile", { path: f })}
                >
                  <FileCode size={12} /> {f}
                </button>
              </li>
            ))}
            {files.length > MAX_LIST && <li className="arch-detail__muted">{t("detail.more", { count: files.length - MAX_LIST })}</li>}
          </ul>
        )}
      </section>

      {node.routes.length > 0 && (
        <section>
          <h4>{t("detail.routes")}</h4>
          <ul className="arch-detail__list">
            {node.routes.slice(0, MAX_LIST).map((r) => (
              <li key={`${r.method} ${r.path} ${r.file}:${r.line}`}>
                <button
                  type="button"
                  className="arch-detail__link arch-mono"
                  onClick={() => routeFile(r.file, r.line)}
                  title={t("detail.openFile", { path: `${r.file}:${r.line}` })}
                >
                  <span className="arch-detail__method">{r.method}</span> {r.path}
                  <span className="arch-detail__muted">{r.framework}</span>
                </button>
              </li>
            ))}
            {node.routes.length > MAX_LIST && (
              <li className="arch-detail__muted">{t("detail.more", { count: node.routes.length - MAX_LIST })}</li>
            )}
          </ul>
        </section>
      )}

      {node.dependencies.length > 0 && (
        <section>
          <h4>{t("detail.dependencies")}</h4>
          <p className="arch-detail__deps arch-mono">{node.dependencies.join(", ")}</p>
        </section>
      )}

      {outgoing.length > 0 && (
        <section>
          <h4>{t("detail.outgoing")}</h4>
          <ul className="arch-detail__list">{edgeList(outgoing, "out")}</ul>
        </section>
      )}
      {incoming.length > 0 && (
        <section>
          <h4>{t("detail.incoming")}</h4>
          <ul className="arch-detail__list">{edgeList(incoming, "in")}</ul>
        </section>
      )}
    </aside>
  );
}
