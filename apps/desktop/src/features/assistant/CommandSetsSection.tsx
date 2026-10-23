/**
 * Settings → Assistant (ADR-0035): command sets (built-in catalogue + user
 * sets like LAITIS) — enable / disable, import / export, create and edit
 * sets with a command editor (phrases with {slots}, multi-step actions,
 * conditions, per-app context), test-run, and a catalogue browser.
 */
import type { AssistantCommand, CommandSet, CommandSetSummary, CommandSummary, ComputerAction, ComputerStatus, SafetyLevel } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { ArrowDown, ArrowUp, Download, FlaskConical, Pencil, Plus, Trash2, Upload } from "lucide-react";
import { type ChangeEvent, useCallback, useEffect, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { currentLanguage, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { SettingGroup, SettingRow, Switch } from "../settings/controls";
import { applyResult, stepText } from "./api";
import { ACTION_TYPES, type ActionType, defaultStep, slotNum, STEP_FIELDS } from "./steps";
import "./assistant.css";

const SAFETY: readonly SafetyLevel[] = ["Safe", "Confirm", "Dangerous"];

function slug(s: string): string {
  const base = s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  return base || `set-${Date.now().toString(36)}`;
}

function download(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 2000);
}

export function CommandSetsSection() {
  const { t } = useT("assistant");
  const [status, setStatus] = useState<ComputerStatus | null>(null);
  const [sets, setSets] = useState<CommandSetSummary[]>([]);
  const [editing, setEditing] = useState<CommandSet | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const load = useCallback(async () => {
    try {
      const c = getClient();
      const [st, ls] = await Promise.all([c.call("computer.status", {}), c.call("computer.listSets", {})]);
      setStatus(st);
      setSets(ls.sets);
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);
  useEffect(() => void load(), [load]);

  const act = async (id: string, fn: () => Promise<unknown>) => {
    setBusy(id);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  const onImport = async (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (!f) return;
    const json = await f.text();
    await act("import", () => getClient().call("computer.importSet", { json, replace: false }));
  };

  const newSet = () =>
    setEditing({
      id: slug(t("sets.newName")),
      name: t("sets.newName"),
      description: "",
      enabled: true,
      builtin: false,
      commands: [],
    });

  return (
    <div className="as-sets">
      <SettingGroup title={t("settings.status")}>
        <SettingRow
          title={status?.available ? t("settings.available", { backend: backendName(status.backend) }) : t("settings.unavailable")}
          description={
            status
              ? t("settings.counts", { catalogue: status.catalogueCommands, user: status.userCommands, sets: status.enabledSets, apps: status.indexedApps })
              : ""
          }
        >
          {status && !status.available && <span className="as-error">{status.unavailableReason}</span>}
        </SettingRow>
        <SettingRow title={t("settings.safetyTitle")} description={t("settings.safetyText")}>
          <span />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("sets.title")}>
        <div className="as-toolbar">
          <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={newSet}>
            {t("sets.new")}
          </Button>
          <Button size="sm" leftIcon={<Upload size={14} />} loading={busy === "import"} onClick={() => fileRef.current?.click()}>
            {t("sets.import")}
          </Button>
          <input ref={fileRef} type="file" accept=".json,application/json" hidden onChange={(e) => void onImport(e)} aria-label={t("sets.import")} />
        </div>
        {error && <div className="as-error">{error}</div>}
        {sets.map((s) => (
          <div key={s.id} className="as-set">
            <div className="as-set__main">
              <div className="as-set__name">
                {s.name} {s.builtin && <Badge tone="info">{t("sets.builtin")}</Badge>}{" "}
                {s.context && <Badge tone="violet">{t("sets.contextual")}</Badge>}
              </div>
              <div className="as-set__desc">{t("sets.commandCount", { count: s.commandCount })}{s.description ? ` · ${s.description}` : ""}</div>
            </div>
            <Switch
              checked={s.enabled}
              busy={busy === `en:${s.id}`}
              label={t("sets.enabled", { name: s.name })}
              onChange={(v) => void act(`en:${s.id}`, () => getClient().call("computer.setEnabled", { setId: s.id, enabled: v }))}
            />
            <IconButton
              label={t("sets.export")}
              icon={<Download size={14} />}
              onClick={() =>
                void act(`ex:${s.id}`, async () => {
                  const r = await getClient().call("computer.exportSet", { setId: s.id });
                  download(r.fileName, r.json);
                })
              }
            />
            {!s.builtin && (
              <>
                <IconButton
                  label={t("sets.edit")}
                  icon={<Pencil size={14} />}
                  onClick={() => void getClient().call("computer.getSet", { setId: s.id }).then((r) => setEditing(r.set), (e: unknown) => setError(describeError(e)))}
                />
                <IconButton
                  label={t("sets.delete")}
                  variant="danger"
                  icon={<Trash2 size={14} />}
                  onClick={() => {
                    if (window.confirm(t("sets.deleteConfirm", { name: s.name }))) void act(`del:${s.id}`, () => getClient().call("computer.deleteSet", { setId: s.id }));
                  }}
                />
              </>
            )}
          </div>
        ))}
      </SettingGroup>

      {editing && (
        <SetEditor
          set={editing}
          onCancel={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void load();
          }}
        />
      )}

      <CatalogueBrowser />
    </div>
  );
}

function SetEditor({ set, onCancel, onSaved }: { set: CommandSet; onCancel: () => void; onSaved: () => void }) {
  const { t } = useT("assistant");
  const [draft, setDraft] = useState<CommandSet>(set);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ctx = draft.context?.processes.join(", ") ?? "";

  const updateCmd = (i: number, c: AssistantCommand) => setDraft((d) => ({ ...d, commands: d.commands.map((x, j) => (j === i ? c : x)) }));
  const addCmd = () =>
    setDraft((d) => ({
      ...d,
      commands: [
        ...d.commands,
        { id: `cmd-${Date.now().toString(36)}`, category: "custom", title: { en: "", ru: "" }, phrases: [""], steps: [defaultStep("Launch")], safety: "Safe" },
      ],
    }));

  const save = async () => {
    setSaving(true);
    try {
      const clean: CommandSet = {
        ...draft,
        commands: draft.commands.map((c) => ({ ...c, phrases: c.phrases.map((p) => p.trim()).filter(Boolean) })),
      };
      await getClient().call("computer.saveSet", { set: clean });
      onSaved();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="as-editor" aria-label={t("editor.title")}>
      <div className="as-row">
        <label className="as-field">
          {t("editor.name")}
          <input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        </label>
        <label className="as-field">
          {t("editor.id")}
          <input value={draft.id} onChange={(e) => setDraft({ ...draft, id: e.target.value })} />
        </label>
      </div>
      <label className="as-field">
        {t("editor.description")}
        <input value={draft.description} onChange={(e) => setDraft({ ...draft, description: e.target.value })} />
      </label>
      <label className="as-field">
        {t("editor.context")}
        <input
          value={ctx}
          placeholder={t("editor.contextPlaceholder")}
          onChange={(e) => {
            const processes = e.target.value
              .split(",")
              .map((s) => s.trim())
              .filter(Boolean);
            setDraft({ ...draft, context: processes.length ? { processes, titleContains: [] } : undefined });
          }}
        />
      </label>
      {draft.commands.map((c, i) => (
        <CommandEditor
          key={i}
          command={c}
          onChange={(nc) => updateCmd(i, nc)}
          onRemove={() => setDraft((d) => ({ ...d, commands: d.commands.filter((_, j) => j !== i) }))}
        />
      ))}
      <div className="as-toolbar">
        <Button size="sm" leftIcon={<Plus size={14} />} onClick={addCmd}>
          {t("editor.addCommand")}
        </Button>
        <span style={{ flex: 1 }} />
        <Button size="sm" variant="ghost" onClick={onCancel}>
          {t("editor.cancel")}
        </Button>
        <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
          {t("editor.save")}
        </Button>
      </div>
      {error && <div className="as-error">{error}</div>}
    </div>
  );
}

function CommandEditor({ command, onChange, onRemove }: { command: AssistantCommand; onChange: (c: AssistantCommand) => void; onRemove: () => void }) {
  const { t } = useT("assistant");
  const [result, setResult] = useState<string | null>(null);
  const setStep = (i: number, s: ComputerAction) => onChange({ ...command, steps: command.steps.map((x, j) => (j === i ? s : x)) });
  const move = (i: number, d: -1 | 1) => {
    const steps = [...command.steps];
    const j = i + d;
    if (j < 0 || j >= steps.length) return;
    [steps[i], steps[j]] = [steps[j] as ComputerAction, steps[i] as ComputerAction];
    onChange({ ...command, steps });
  };
  const test = async () => {
    try {
      const r = await getClient().call("computer.execute", { title: command.title.en || command.id, steps: command.steps, slots: {} });
      setResult(applyResult(r).reply);
    } catch (e) {
      setResult(describeError(e));
    }
  };
  return (
    <div className="as-cmd">
      <div className="as-row">
        <label className="as-field">
          {t("editor.commandTitle")}
          <input value={command.title.en} onChange={(e) => onChange({ ...command, title: { en: e.target.value, ru: e.target.value } })} />
        </label>
        <label className="as-field">
          {t("editor.safety")}
          <select value={command.safety} onChange={(e) => onChange({ ...command, safety: e.target.value as SafetyLevel })}>
            {SAFETY.map((s) => (
              <option key={s} value={s}>
                {t(`safety.${s}`)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label className="as-field">
        {t("editor.phrases")}
        <textarea value={command.phrases.join("\n")} placeholder={t("editor.phrasesPlaceholder")} onChange={(e) => onChange({ ...command, phrases: e.target.value.split("\n") })} />
      </label>
      <div className="as-field">{t("editor.steps")}</div>
      {command.steps.map((s, i) => (
        <StepRow key={i} n={i + 1} step={s} onChange={(ns) => setStep(i, ns)} onRemove={() => onChange({ ...command, steps: command.steps.filter((_, j) => j !== i) })} onMove={(d) => move(i, d)} />
      ))}
      <div className="as-toolbar">
        <Button size="sm" leftIcon={<Plus size={13} />} onClick={() => onChange({ ...command, steps: [...command.steps, defaultStep("Keys")] })}>
          {t("editor.addStep")}
        </Button>
        <Button size="sm" leftIcon={<FlaskConical size={13} />} onClick={() => void test()}>
          {t("editor.test")}
        </Button>
        <span style={{ flex: 1 }} />
        <Button size="sm" variant="ghost" leftIcon={<Trash2 size={13} />} onClick={onRemove}>
          {t("editor.removeCommand")}
        </Button>
      </div>
      {result && <div className="as-reply">{result}</div>}
    </div>
  );
}

function StepRow({ n, step, onChange, onRemove, onMove }: { n: number; step: ComputerAction; onChange: (s: ComputerAction) => void; onRemove: () => void; onMove: (d: -1 | 1) => void }) {
  const { t } = useT("assistant");
  const values = step as unknown as Record<string, unknown>;
  const set = (key: string, v: unknown) => {
    const next: Record<string, unknown> = { ...values };
    if (v === undefined || v === "") delete next[key];
    else next[key] = v;
    onChange(next as unknown as ComputerAction);
  };
  return (
    <div className="as-step" title={stepText(step)}>
      <span className="as-step__n">{n}</span>
      <select value={step.type} aria-label={t("editor.stepType")} onChange={(e) => onChange(defaultStep(e.target.value as ActionType))}>
        {ACTION_TYPES.map((a) => (
          <option key={a} value={a}>
            {t(`actions.${a}`)}
          </option>
        ))}
      </select>
      {STEP_FIELDS[step.type].map((f) => {
        const label = t(`fields.${f.key}`, { defaultValue: f.key });
        const v = values[f.key];
        if (f.kind === "bool")
          return (
            <label key={f.key}>
              <input type="checkbox" checked={v === true} onChange={(e) => set(f.key, e.target.checked)} /> {label}
            </label>
          );
        if (f.kind === "json")
          return (
            <textarea
              key={f.key}
              aria-label={label}
              placeholder={label}
              defaultValue={JSON.stringify(v ?? null)}
              onBlur={(e) => {
                try {
                  set(f.key, JSON.parse(e.target.value));
                } catch {
                  /* keep the last valid value */
                }
              }}
            />
          );
        if (typeof f.kind === "object")
          return (
            <select key={f.key} aria-label={label} value={String(v ?? f.kind.options[0])} onChange={(e) => set(f.key, e.target.value)}>
              {f.kind.options.map((o) => (
                <option key={o} value={o}>
                  {t(`options.${o}`, { defaultValue: o })}
                </option>
              ))}
            </select>
          );
        return (
          <input
            key={f.key}
            aria-label={label}
            placeholder={f.optional ? t("editor.optional", { field: label }) : label}
            value={v === undefined ? "" : String(v)}
            onChange={(e) => set(f.key, f.kind === "num" ? (e.target.value === "" ? undefined : slotNum(e.target.value)) : e.target.value)}
          />
        );
      })}
      <IconButton size="sm" label={t("editor.moveUp")} icon={<ArrowUp size={12} />} onClick={() => onMove(-1)} />
      <IconButton size="sm" label={t("editor.moveDown")} icon={<ArrowDown size={12} />} onClick={() => onMove(1)} />
      <IconButton size="sm" label={t("editor.removeStep")} variant="danger" icon={<Trash2 size={12} />} onClick={onRemove} />
    </div>
  );
}

function CatalogueBrowser() {
  const { t } = useT("assistant");
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("");
  const [rows, setRows] = useState<CommandSummary[]>([]);
  const [total, setTotal] = useState(0);
  const [cats, setCats] = useState<string[]>([]);
  useEffect(() => {
    let live = true;
    const h = window.setTimeout(() => {
      getClient()
        .call("computer.catalogue", { query, lang: currentLanguage(), limit: 100, offset: 0, ...(category ? { category } : {}) })
        .then((r) => {
          if (!live) return;
          setRows(r.commands);
          setTotal(r.total);
          setCats(r.categories.map((c) => c.category).filter(Boolean));
        })
        .catch(() => undefined);
    }, 150);
    return () => {
      live = false;
      window.clearTimeout(h);
    };
  }, [query, category]);
  return (
    <SettingGroup title={t("catalogue.title", { count: total })}>
      <div className="as-row">
        <label className="as-field">
          {t("catalogue.search")}
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("catalogue.searchPlaceholder")} />
        </label>
        <label className="as-field">
          {t("catalogue.category")}
          <select value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">{t("catalogue.all")}</option>
            {cats.map((c) => (
              <option key={c} value={c}>
                {t(`categories.${c}`, { defaultValue: c })}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="as-cat" role="list">
        {rows.map((r) => (
          <div key={`${r.setId}/${r.commandId}`} className="as-cat__row" role="listitem">
            <span className="as-cat__title">
              {r.title} {r.safety !== "Safe" && <Badge tone="warning">{t(`safety.${r.safety}`)}</Badge>}
            </span>
            <span className="as-cat__phrases">{r.phrases.join(" · ")}</span>
          </div>
        ))}
      </div>
    </SettingGroup>
  );
}

/** Display name of a computer-control backend id (`windows` → `Windows`). */
function backendName(id: string): string {
  const known: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
  return known[id] ?? id;
}
