/**
 * Provider + model chooser. Models come from `providers.models` (cached per
 * provider); any model id can also be typed.
 */
import type { Model, ModelRef, ProviderView } from "@orkestr/protocol";
import { Input } from "@orkestr/ui";
import { useEffect, useId, useState } from "react";

import { Select } from "./controls";
import { listModels } from "./store";

const cache = new Map<string, Promise<Model[]>>();

function modelsOf(providerId: string): Promise<Model[]> {
  let p = cache.get(providerId);
  if (!p) {
    p = listModels(providerId).catch((e: unknown) => {
      cache.delete(providerId);
      throw e;
    });
    cache.set(providerId, p);
  }
  return p;
}

export interface ModelPickerProps {
  label: string;
  value: ModelRef | null;
  onChange: (value: ModelRef | null) => void;
  providers: readonly ProviderView[];
  /** Offer an empty choice. */
  allowNone?: boolean;
}

export function ModelPicker({ label, value, onChange, providers, allowNone = false }: ModelPickerProps) {
  const listId = useId();
  const [models, setModels] = useState<Model[]>([]);
  const providerId = value?.providerId ?? "";

  useEffect(() => {
    let alive = true;
    if (!providerId) {
      setModels([]);
      return undefined;
    }
    modelsOf(providerId)
      .then((m) => alive && setModels(m))
      .catch(() => alive && setModels([]));
    return () => {
      alive = false;
    };
  }, [providerId]);

  const pickProvider = (id: string) => {
    if (!id) {
      onChange(null);
      return;
    }
    const p = providers.find((v) => v.provider.id === id);
    onChange({ providerId: id, model: p?.provider.defaultModel ?? "" });
  };

  return (
    <div className="pv-picker" role="group" aria-label={label}>
      <Select label={`${label} — provider`} value={providerId} onChange={(e) => pickProvider(e.target.value)}>
        {(allowNone || !providerId) && <option value="">{allowNone ? "— none —" : "Choose a provider…"}</option>}
        {providers.map((p) => (
          <option key={p.provider.id} value={p.provider.id} disabled={!p.provider.enabled}>
            {p.provider.name}
            {p.provider.enabled ? "" : " (disabled)"}
          </option>
        ))}
      </Select>
      <Input
        label={`${label} — model`}
        list={listId}
        value={value?.model ?? ""}
        disabled={!providerId}
        placeholder={providerId ? "model id" : "pick a provider first"}
        spellCheck={false}
        onChange={(e) => providerId && onChange({ providerId, model: e.target.value })}
      />
      <datalist id={listId}>
        {models.map((m) => (
          <option key={m.id} value={m.id}>
            {m.displayName}
          </option>
        ))}
      </datalist>
    </div>
  );
}
