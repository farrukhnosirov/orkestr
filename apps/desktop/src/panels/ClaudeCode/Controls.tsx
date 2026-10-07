/**
 * Mode switcher («По умолчанию» / «Принимать правки» / «План» / «Без
 * подтверждений» behind a danger confirmation) and model picker.
 */
import type { ClaudePermissionMode } from "@orkestr/protocol";
import { Button } from "@orkestr/ui";
import { ShieldAlert } from "lucide-react";
import { useState } from "react";

import { Overlay } from "../../components/Overlay";
import { useT } from "../../i18n";
import type { ModelPickerProps, ModeSwitcherProps } from "./types";

const MODES: ClaudePermissionMode[] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/** Aliases offered before the CLI reported its own list. */
const FALLBACK_MODELS = ["default", "sonnet", "opus", "haiku"];

export function ModeSwitcher({ mode, disabled, onChange }: ModeSwitcherProps) {
  const { t } = useT(["claudeCode", "common", "security"]);
  const [confirm, setConfirm] = useState(false);
  return (
    <>
      <select
        className="cc-select"
        value={mode}
        disabled={disabled}
        aria-label={t("mode.label")}
        title={t(`mode.hint.${mode}`)}
        onChange={(e) => {
          const next = e.target.value as ClaudePermissionMode;
          if (next === "bypassPermissions") setConfirm(true);
          else onChange(next);
        }}
      >
        {MODES.map((m) => (
          <option key={m} value={m}>
            {t(`mode.${m}`)}
          </option>
        ))}
      </select>
      {confirm && (
        <Overlay onDismiss={() => setConfirm(false)}>
          <div className="dialog cc-danger-dialog" role="alertdialog" aria-labelledby="cc-bypass-title">
            <h2 id="cc-bypass-title" className="cc-danger-dialog__title">
              <ShieldAlert size={18} /> {t("bypass.title")}
            </h2>
            <p>{t("bypass.body")}</p>
            <p className="cc-muted">{t("bypass.note")}</p>
            <div className="cc-danger-dialog__actions">
              <Button variant="ghost" onClick={() => setConfirm(false)}>
                {t("common:actions.cancel")}
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  setConfirm(false);
                  onChange("bypassPermissions", true);
                }}
              >
                {t("bypass.confirm")}
              </Button>
            </div>
          </div>
        </Overlay>
      )}
    </>
  );
}

export function ModelPicker({ value, models, disabled, onChange }: ModelPickerProps) {
  const { t } = useT(["claudeCode", "common", "security"]);
  const options = models.length > 0 ? models.map((m) => ({ value: m.value, label: m.displayName })) : FALLBACK_MODELS.map((v) => ({ value: v, label: v === "default" ? t("model.default") : v }));
  if (!options.some((o) => o.value === value)) options.push({ value, label: value });
  return (
    <select className="cc-select" value={value} disabled={disabled} aria-label={t("model.label")} onChange={(e) => onChange(e.target.value)}>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
