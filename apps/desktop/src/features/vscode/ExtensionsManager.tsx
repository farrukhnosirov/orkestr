/**
 * Extensions of Orkestr's VS Code, managed from Orkestr (ADR-0043): the
 * installed list (from the server's extensions directory, with names and
 * icons from the manifests), install from Open VSX by id or from a local
 * `.vsix`, update all, enable / disable (applied on the next server start)
 * and uninstall. Browsing the Open VSX catalog lives in the Extensions panel.
 */
import type { VsCodeExtension, VsCodeExtensionCommands } from "@orkestr/protocol";
import { Badge, Button, IconButton, Input, Spinner } from "@orkestr/ui";
import { AlertTriangle, Blocks, FileArchive, LayoutPanelLeft, PackagePlus, RefreshCw, RotateCcw, Settings2, Trash2 } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { canPickPaths, pickFile } from "../../lib/tauri";
import { ActionDialog } from "../files/Dialog";
import { useLauncherStore } from "./launcher";
import { useVsCodeStore } from "./store";

/** The bridge is Orkestr's own plumbing, not a user extension. */
export const BRIDGE_EXTENSION_ID = "orkestr.orkestr-bridge";

/** Suggested Open VSX extensions (one click). */
export const SUGGESTED_EXTENSIONS: readonly {
  id: string;
  labelKey: "extensions.claude" | "extensions.rust" | "extensions.python" | "extensions.gitlens";
}[] = [
  { id: "anthropic.claude-code", labelKey: "extensions.claude" },
  { id: "rust-lang.rust-analyzer", labelKey: "extensions.rust" },
  { id: "ms-python.python", labelKey: "extensions.python" },
  { id: "eamodio.gitlens", labelKey: "extensions.gitlens" },
];

/** User-visible extensions (the bridge left out), disabled ones last. */
export function visibleExtensions(list: readonly VsCodeExtension[] | null): VsCodeExtension[] {
  return (list ?? [])
    .filter((e) => e.id !== BRIDGE_EXTENSION_ID)
    .slice()
    .sort((a, b) => Number(a.disabled) - Number(b.disabled) || a.id.localeCompare(b.id));
}

function ExtensionRow({
  ext,
  meta,
  busy,
  disabledActions,
  onToggle,
  onUninstall,
}: {
  ext: VsCodeExtension;
  meta: VsCodeExtensionCommands | undefined;
  busy: boolean;
  disabledActions: boolean;
  onToggle: () => void;
  onUninstall: () => void;
}) {
  const { t } = useT("vscode");
  const launch = useLauncherStore((s) => s.launch);
  const name = meta?.displayName || ext.id;
  const view = meta?.viewContainers[0];
  return (
    <li className={ext.disabled ? "vsc-ext vsc-ext--disabled" : "vsc-ext"}>
      {meta?.icon ? (
        <img className="vsc-ext__icon" src={meta.icon} alt="" aria-hidden="true" />
      ) : (
        <span className="vsc-ext__icon vsc-ext__icon--none" aria-hidden="true">
          <Blocks size={18} />
        </span>
      )}
      <div className="vsc-ext__main">
        <div className="vsc-row">
          <strong className="vsc-ext__name">{name}</strong>
          {ext.version && <span className="vsc-muted">{t("manager.version", { version: ext.version })}</span>}
          {ext.disabled && <Badge tone="neutral">{t("manager.disabled")}</Badge>}
          {ext.restartRequired && <Badge tone="warning">{t("manager.restartRequired")}</Badge>}
        </div>
        <code className="vsc-ext__id">{ext.id}</code>
        {meta?.description && <p className="vsc-ext__desc">{meta.description}</p>}
      </div>
      <div className="vsc-ext__actions">
        {busy && <Spinner size={14} />}
        {view && !ext.disabled && (
          <IconButton
            label={t("manager.openView", { name: view.title })}
            icon={<LayoutPanelLeft size={14} />}
            tooltip="bottom"
            onClick={() => void launch({ kind: "view", extensionId: ext.id, containerId: view.id, title: view.title })}
          />
        )}
        {meta?.hasSettings && !ext.disabled && (
          <IconButton
            label={t("manager.settings")}
            icon={<Settings2 size={14} />}
            tooltip="bottom"
            onClick={() => void launch({ kind: "settings", extensionId: ext.id, title: name })}
          />
        )}
        <Button size="sm" variant="secondary" disabled={disabledActions} onClick={onToggle}>
          {ext.disabled ? t("manager.enable") : t("manager.disable")}
        </Button>
        <IconButton
          label={t("manager.uninstall")}
          icon={<Trash2 size={14} />}
          tooltip="bottom"
          variant="danger"
          disabled={disabledActions}
          onClick={onUninstall}
        />
      </div>
    </li>
  );
}

export function ExtensionsManager({ projectId, installed }: { projectId: string | null; installed: boolean }) {
  const { t } = useT("vscode");
  const extensions = useVsCodeStore((s) => s.extensions);
  const error = useVsCodeStore((s) => s.extensionsError);
  const installing = useVsCodeStore((s) => s.installingExtension);
  const managing = useVsCodeStore((s) => s.managingExtension);
  const running = useVsCodeStore((s) => s.status?.state === "running");
  const busyStart = useVsCodeStore((s) => s.busy === "start" || s.busy === "stop");
  const catalog = useLauncherStore((s) => s.extensions);
  const [id, setId] = useState("");
  const [vsixPath, setVsixPath] = useState("");
  const [changed, setChanged] = useState(false);
  const [removing, setRemoving] = useState<VsCodeExtension | null>(null);

  useEffect(() => {
    if (!installed) return;
    void useVsCodeStore.getState().loadExtensions();
    void useLauncherStore.getState().load();
  }, [installed]);

  const meta = useMemo(() => new Map((catalog ?? []).map((e) => [e.id, e])), [catalog]);
  const list = visibleExtensions(extensions);
  const anyBusy = installing !== null || managing !== null;
  const restartNeeded = list.some((e) => e.restartRequired);

  const after = (ok: boolean) => {
    if (!ok) return;
    setChanged(true);
    void useLauncherStore.getState().load(true);
  };
  const install = async (value: string) => {
    const ok = await useVsCodeStore.getState().installExtension(value);
    if (ok) setId("");
    after(ok);
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void install(id);
  };
  const installVsix = async () => {
    let path = vsixPath.trim();
    if (!path && canPickPaths()) path = (await pickFile({ title: t("manager.vsixPick") })) ?? "";
    if (!path) return;
    const ok = await useVsCodeStore.getState().installVsix(path);
    if (ok) setVsixPath("");
    after(ok);
  };
  const has = (ext: string) => list.some((x) => x.id === ext);

  if (!installed) {
    return (
      <section className="vsc-card">
        <h4 className="vsc-row">
          <Blocks size={14} aria-hidden="true" />
          {t("extensions.title")}
        </h4>
        <p className="vsc-muted">{t("extensions.notInstalled")}</p>
      </section>
    );
  }

  return (
    <section className="vsc-card vsc-card--wide vsc-manager" aria-labelledby="vsc-extensions-title">
      <div className="vsc-row">
        <h4 id="vsc-extensions-title" className="vsc-row">
          <Blocks size={14} aria-hidden="true" />
          {t("manager.title", { n: list.length })}
        </h4>
        <span className="vsc-spacer" />
        <Button
          size="sm"
          variant="ghost"
          leftIcon={<RefreshCw size={14} />}
          loading={managing === "*" && installing === null}
          disabled={anyBusy}
          onClick={() => void useVsCodeStore.getState().updateExtensions().then(after)}
        >
          {t("manager.updateAll")}
        </Button>
        <IconButton
          label={t("extensions.refresh")}
          icon={<RotateCcw size={14} />}
          tooltip="bottom"
          onClick={() => {
            void useVsCodeStore.getState().loadExtensions();
            void useLauncherStore.getState().load(true);
          }}
        />
      </div>
      <p className="vsc-muted">{t("extensions.subtitle")}</p>

      <form className="vsc-row" onSubmit={submit}>
        <Input
          aria-label={t("extensions.idLabel")}
          placeholder={t("extensions.idPlaceholder")}
          value={id}
          maxLength={200}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setId(e.target.value)}
          disabled={anyBusy}
        />
        <Button type="submit" variant="primary" leftIcon={<PackagePlus size={14} />} loading={installing !== null} disabled={!id.trim() || anyBusy}>
          {t("extensions.install")}
        </Button>
      </form>
      <div className="vsc-row">
        {!canPickPaths() && (
          <Input
            aria-label={t("manager.vsixLabel")}
            placeholder={t("manager.vsixPlaceholder")}
            value={vsixPath}
            spellCheck={false}
            onChange={(e) => setVsixPath(e.target.value)}
            disabled={anyBusy}
          />
        )}
        <Button
          size="sm"
          variant="secondary"
          leftIcon={<FileArchive size={14} />}
          disabled={anyBusy || (!canPickPaths() && !vsixPath.trim())}
          onClick={() => void installVsix()}
        >
          {t("manager.vsix")}
        </Button>
        {SUGGESTED_EXTENSIONS.filter((x) => !has(x.id)).map((x) => (
          <Button key={x.id} size="sm" variant="ghost" leftIcon={<PackagePlus size={14} />} disabled={anyBusy} onClick={() => void install(x.id)}>
            {t(x.labelKey)}
          </Button>
        ))}
      </div>

      {installing && (
        <p className="vsc-row vsc-muted" role="status">
          <Spinner size={14} /> {t("extensions.installing", { id: installing })}
        </p>
      )}
      {error && (
        <div className="vsc-alert" role="alert">
          <AlertTriangle size={14} aria-hidden="true" />
          <span>{error}</span>
        </div>
      )}
      {restartNeeded && (
        <div className="vsc-notice" role="status">
          <span>{running ? t("manager.restartHint") : t("manager.appliesOnStart")}</span>
          {running && (
            <Button size="sm" variant="primary" leftIcon={<RotateCcw size={14} />} loading={busyStart} onClick={() => {
                // The restart reloads the workbench too: no separate reload hint afterwards.
                setChanged(false);
                void useVsCodeStore.getState().restart(projectId);
              }}>
              {t("manager.restartNow")}
            </Button>
          )}
        </div>
      )}
      {changed && running && !restartNeeded && (
        <div className="vsc-notice" role="status">
          <span>{t("extensions.reloadHint")}</span>
          <Button size="sm" variant="secondary" leftIcon={<RefreshCw size={14} />} onClick={() => useVsCodeStore.getState().reloadEmbedded()}>
            {t("run.reload")}
          </Button>
        </div>
      )}

      {extensions === null ? (
        <Spinner size={14} />
      ) : list.length === 0 ? (
        <p className="vsc-muted">{t("extensions.none")}</p>
      ) : (
        <ul className="vsc-ext-list" aria-label={t("manager.title", { n: list.length })}>
          {list.map((x) => (
            <ExtensionRow
              key={x.id}
              ext={x}
              meta={meta.get(x.id)}
              busy={managing === x.id}
              disabledActions={anyBusy}
              onToggle={() => void useVsCodeStore.getState().setExtensionEnabled(x.id, x.disabled).then(after)}
              onUninstall={() => setRemoving(x)}
            />
          ))}
        </ul>
      )}
      <p className="vsc-muted">{t("extensions.browseHint")}</p>

      {removing && (
        <ActionDialog
          title={t("manager.uninstallTitle", { name: meta.get(removing.id)?.displayName ?? removing.id })}
          icon={<Trash2 size={18} />}
          tone="danger"
          onCancel={() => setRemoving(null)}
          actions={[
            { id: "cancel", label: t("manager.cancel"), variant: "ghost", autoFocus: true, onSelect: () => setRemoving(null) },
            {
              id: "remove",
              label: t("manager.uninstall"),
              variant: "danger",
              onSelect: () => {
                const target = removing;
                setRemoving(null);
                void useVsCodeStore.getState().uninstallExtension(target.id).then(after);
              },
            },
          ]}
        >
          <p>{t("manager.uninstallBody")}</p>
        </ActionDialog>
      )}
    </section>
  );
}
