/**
 * Compact model picker: По умолчанию / Sonnet / Opus / Haiku, the models the
 * CLI reported, and «Другая модель…» for a full model id.
 */
import { Cpu } from "lucide-react";
import { useState } from "react";

import { useT } from "../../../i18n";
import type { ModelPickerProps } from "../types";

const ALIASES = ["default", "sonnet", "opus", "haiku"] as const;
const CUSTOM = "__custom__";

export function ModelPicker({ value, models, disabled, onChange }: ModelPickerProps) {
  const { t } = useT("claudeCodeComposer");
  const [custom, setCustom] = useState<string | null>(null);

  const extra = models.filter((m) => !(ALIASES as readonly string[]).includes(m.value));
  const known = (ALIASES as readonly string[]).includes(value) || extra.some((m) => m.value === value);

  if (custom !== null) {
    const commit = () => {
      const id = custom.trim();
      if (id) onChange(id);
      setCustom(null);
    };
    return (
      <form
        className="ccx-model ccx-model--custom"
        onSubmit={(e) => {
          e.preventDefault();
          commit();
        }}
      >
        <input
          className="ccx-model__input"
          value={custom}
          autoFocus
          aria-label={t("model.customLabel")}
          placeholder={t("model.customPlaceholder")}
          onChange={(e) => setCustom(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              setCustom(null);
            }
          }}
        />
        <button type="submit" className="ccx-model__btn" disabled={!custom.trim()}>
          {t("model.apply")}
        </button>
        <button type="button" className="ccx-model__btn ccx-model__btn--ghost" onClick={() => setCustom(null)}>
          {t("model.cancel")}
        </button>
      </form>
    );
  }

  return (
    <label className="ccx-model" title={t("model.label")}>
      <Cpu size={13} aria-hidden />
      <select
        className="ccx-model__select"
        value={value}
        disabled={disabled}
        aria-label={t("model.label")}
        onChange={(e) => {
          if (e.target.value === CUSTOM) setCustom(known ? "" : value);
          else onChange(e.target.value);
        }}
      >
        {ALIASES.map((a) => (
          <option key={a} value={a}>
            {t(`model.${a}`)}
          </option>
        ))}
        {extra.map((m) => (
          <option key={m.value} value={m.value} title={m.description ?? undefined}>
            {m.displayName}
          </option>
        ))}
        {!known && value && <option value={value}>{value}</option>}
        <option value={CUSTOM}>{t("model.custom")}</option>
      </select>
    </label>
  );
}
