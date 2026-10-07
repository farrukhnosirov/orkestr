/**
 * Orkestr trash: copies of files deleted in the Files panel (`files.trash`),
 * restorable with `files.restore`. Restoring over an existing file asks first
 * and only then retries with `overwrite: true`.
 */
import type { TrashItem } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { ArchiveRestore, Check } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { ActionDialog } from "../../features/files/Dialog";
import { errorInfo } from "../../features/files/rpc";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { formatBytes, formatRelativeTime } from "../../i18n/format";

interface Props {
  projectId: string;
  onClose: () => void;
  /** Called with the restored workspace-relative path. */
  onRestored: (path: string) => void;
}

export function TrashDialog({ projectId, onClose, onRestored }: Props) {
  const { t } = useT(["files", "common"]);
  const [items, setItems] = useState<TrashItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmReplace, setConfirmReplace] = useState<TrashItem | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const res = await getClient().call("files.trash", { projectId });
      setItems(res.items);
    } catch (e) {
      setItems([]);
      setError(describeError(e));
    }
  }, [projectId]);

  useEffect(() => {
    void load();
  }, [load]);

  const restore = async (item: TrashItem, overwrite: boolean) => {
    setBusy(item.id);
    setError(null);
    setDone(null);
    try {
      const res = await getClient().call("files.restore", { projectId, trashId: item.id, overwrite });
      setConfirmReplace(null);
      setDone(t("trash.restored", { path: res.path }));
      onRestored(res.path);
      await load();
    } catch (e) {
      if (!overwrite && errorInfo(e).reason === "exists") setConfirmReplace(item);
      else setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  if (confirmReplace) {
    return (
      <ActionDialog
        title={t("trash.replaceTitle", { path: confirmReplace.path })}
        subtitle={t("trash.replaceDescription")}
        icon={<ArchiveRestore size={18} />}
        tone="warning"
        error={error}
        onCancel={() => busy === null && setConfirmReplace(null)}
        actions={[
          { id: "cancel", label: t("common:actions.cancel"), variant: "ghost", autoFocus: true, onSelect: () => setConfirmReplace(null) },
          {
            id: "replace",
            label: t("trash.replace"),
            variant: "danger",
            icon: <ArchiveRestore size={14} />,
            busy: busy === confirmReplace.id,
            onSelect: () => void restore(confirmReplace, true),
          },
        ]}
      />
    );
  }

  return (
    <ActionDialog
      title={t("trash.title")}
      subtitle={t("trash.subtitle")}
      icon={<ArchiveRestore size={18} />}
      error={error}
      onCancel={onClose}
      actions={[{ id: "close", label: t("common:actions.close"), variant: "secondary", autoFocus: true, onSelect: onClose }]}
    >
      {done && (
        <p className="fx-dialog__text fx-trash__done" role="status">
          <Check size={13} /> {done}
        </p>
      )}
      {items === null ? (
        <div className="fx-trash__loading">
          <Spinner size={14} /> {t("trash.loading")}
        </div>
      ) : items.length === 0 ? (
        <p className="fx-dialog__text">{t("trash.empty")}</p>
      ) : (
        <ul className="fx-trash" aria-label={t("trash.list")}>
          {items.map((item) => (
            <li key={item.id} className={clsx("fx-trash__item", item.restored && "fx-trash__item--restored")}>
              <div className="fx-trash__main">
                <span className="fx-trash__path" title={item.path}>
                  {item.path}
                </span>
                <span className="fx-trash__meta">
                  {t("trash.meta", {
                    time: formatRelativeTime(item.deletedAt),
                    size: formatBytes(item.size),
                    actor: t(`trash.actor.${item.actor}`),
                  })}
                  {item.restored && <Badge tone="success">{t("trash.restoredBadge")}</Badge>}
                </span>
              </div>
              <Button
                size="sm"
                variant="secondary"
                leftIcon={<ArchiveRestore size={13} />}
                loading={busy === item.id}
                disabled={busy !== null && busy !== item.id}
                aria-label={t("trash.restoreItem", { path: item.path })}
                onClick={() => void restore(item, false)}
              >
                {t("trash.restore")}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </ActionDialog>
  );
}
