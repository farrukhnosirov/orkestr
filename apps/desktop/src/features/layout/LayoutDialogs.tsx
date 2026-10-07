/** Save-as / rename / import dialogs of the layout feature. */
import { Button, Input } from "@orkestr/ui";
import { FileUp, LayoutTemplate, X } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../i18n";
import * as layout from "./controller";
import { isValidLayoutName } from "./schema";
import { useLayoutStore } from "./store";
import { Overlay } from "../../components/Overlay";

function NameDialog({ mode, initial, id }: { mode: "saveAs" | "rename"; initial: string; id?: string }) {
  const { t } = useT(["layout", "common"]);
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLInputElement>(null);
  const close = () => !busy && useLayoutStore.getState().patch({ dialog: null });

  useEffect(() => {
    requestAnimationFrame(() => {
      ref.current?.focus();
      ref.current?.select();
    });
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!isValidLayoutName(name)) {
      setError(t("dialogs.nameInvalid"));
      return;
    }
    setBusy(true);
    const ok = mode === "saveAs" ? await layout.saveAs(name) : id ? await layout.renameLayout(id, name.trim()) : false;
    setBusy(false);
    if (ok) useLayoutStore.getState().patch({ dialog: null });
    else setError(t("dialogs.saveFailed"));
  };

  const titleId = "layout-name-dialog-title";
  return (
    <Overlay onDismiss={close}>
      <form className="dialog" role="dialog" aria-modal="true" aria-labelledby={titleId} onSubmit={(e) => void submit(e)} onKeyDown={(e) => e.key === "Escape" && close()}>
        <header className="dialog__header">
          <div className="dialog__icon">
            <LayoutTemplate size={18} />
          </div>
          <div>
            <h2 id={titleId} className="dialog__title">
              {mode === "saveAs" ? t("dialogs.saveTitle") : t("dialogs.renameTitle")}
            </h2>
            <p className="dialog__subtitle">{mode === "saveAs" ? t("dialogs.saveSubtitle") : t("dialogs.renameSubtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <Input
            ref={ref}
            label={t("dialogs.name")}
            placeholder={t("dialogs.namePlaceholder")}
            value={name}
            maxLength={80}
            error={error ?? undefined}
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
          />
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {mode === "saveAs" ? t("dialogs.save") : t("dialogs.rename")}
          </Button>
        </footer>
      </form>
    </Overlay>
  );
}

function ImportDialog() {
  const { t } = useT(["layout", "common"]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const close = () => !busy && useLayoutStore.getState().patch({ dialog: null });

  const onFile = async (file: File | undefined) => {
    if (!file) return;
    if (file.size > 512 * 1024) {
      setError(t("dialogs.importErrors.tooLarge"));
      return;
    }
    setBusy(true);
    setError(null);
    const text = await file.text();
    const result = await layout.importFromText(text);
    setBusy(false);
    if (result.ok) {
      useLayoutStore.getState().patch({ dialog: null });
      return;
    }
    const known = ["json", "format", "version", "invalid", "empty", "core"] as const;
    const code = known.find((k) => k === result.error) ?? "invalid";
    setError(t(`dialogs.importErrors.${code}`));
  };

  return (
    <Overlay onDismiss={close}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="layout-import-title" onKeyDown={(e) => e.key === "Escape" && close()}>
        <header className="dialog__header">
          <div className="dialog__icon">
            <FileUp size={18} />
          </div>
          <div>
            <h2 id="layout-import-title" className="dialog__title">
              {t("dialogs.importTitle")}
            </h2>
            <p className="dialog__subtitle">{t("dialogs.importSubtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <input ref={fileRef} type="file" accept=".json,application/json" hidden onChange={(e) => void onFile(e.target.files?.[0])} />
          {error && (
            <p className="lyt-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" leftIcon={<FileUp size={14} />} loading={busy} onClick={() => fileRef.current?.click()}>
            {t("dialogs.importFile")}
          </Button>
        </footer>
      </div>
    </Overlay>
  );
}

/** Mounted once in App. */
export function LayoutDialogs() {
  const dialog = useLayoutStore((s) => s.dialog);
  if (!dialog) return null;
  if (dialog.kind === "import") return <ImportDialog />;
  if (dialog.kind === "rename") return <NameDialog key={dialog.id} mode="rename" initial={dialog.name} id={dialog.id} />;
  return <NameDialog key="saveAs" mode="saveAs" initial="" />;
}
