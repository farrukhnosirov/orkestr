/**
 * Theme Editor (§29): a glass drawer over the live app. Every valid edit is
 * applied to the whole app immediately (live preview); invalid or illegible
 * drafts are shown in the editor but never applied (§70). Built-in themes are
 * read-only — saving them creates a user theme.
 */
import type { ThemeConfig } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  Boxes,
  Braces,
  CircleDot,
  Download,
  Image,
  Music,
  Palette,
  RotateCcw,
  Save,
  Sparkles,
  Type,
  Upload,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { LucideIcon } from "lucide-react";
import { type ComponentType, useEffect, useRef, useState } from "react";

import { useT } from "../../i18n";
import { Segmented } from "../settings/controls";
import { BackgroundTab, ColorsTab, EffectsTab, MotionTab, OrbTab, SoundsTab, TypographyTab, type TabProps } from "./editorTabs";
import { downloadText } from "./i18nText";
import { exportFileName, savesAsNew } from "./logic";
import { ThemeMiniature } from "./ThemeCard";
import { useVisualsStore } from "./ThemeRuntime";
import { useThemeStore } from "./themeStore";

type TabId = "colors" | "typography" | "effects" | "motion" | "background" | "sounds" | "orb" | "json";

const TABS: ReadonlyArray<{ id: TabId; icon: LucideIcon; component?: ComponentType<TabProps> }> = [
  { id: "colors", icon: Palette, component: ColorsTab },
  { id: "typography", icon: Type, component: TypographyTab },
  { id: "effects", icon: Sparkles, component: EffectsTab },
  { id: "motion", icon: Boxes, component: MotionTab },
  { id: "background", icon: Image, component: BackgroundTab },
  { id: "sounds", icon: Music, component: SoundsTab },
  { id: "orb", icon: CircleDot, component: OrbTab },
  { id: "json", icon: Braces },
];

function JsonTab({ draft, onImport }: { draft: ThemeConfig; onImport: (json: string) => string[] | null }) {
  const { t } = useT("theme");
  const exportJson = useThemeStore((s) => s.exportJson);
  const edit = useThemeStore((s) => s.edit);
  const [text, setText] = useState(() => exportJson(draft));
  const [errors, setErrors] = useState<string[] | null>(null);
  const [copied, setCopied] = useState(false);
  return (
    <div className="th-tab">
      <section className="th-block">
        <label className="th-field">
          <span className="th-field__label">{t("editor.description")}</span>
          <textarea
            className="th-code th-code--prose"
            rows={2}
            maxLength={280}
            value={draft.description}
            onChange={(e) => edit((th) => ({ ...th, description: e.target.value }))}
          />
        </label>
        <label className="th-field">
          <span className="th-field__label">{t("editor.author")}</span>
          <input className="th-input" maxLength={80} value={draft.author} onChange={(e) => edit((th) => ({ ...th, author: e.target.value }))} />
        </label>
      </section>
      <section className="th-block">
        <h4 className="th-block__title">{t("editor.jsonTitle")}</h4>
        <textarea
          className="th-code"
          aria-label={t("editor.jsonTitle")}
          spellCheck={false}
          rows={16}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        {errors && (
          <ul className="th-errors" role="alert">
            {errors.slice(0, 6).map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
        )}
        <div className="th-toolbar">
          <Button size="sm" onClick={() => setErrors(onImport(text))}>
            {t("editor.applyJson")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setText(exportJson(draft))}>
            {t("editor.refreshJson")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              void navigator.clipboard?.writeText(exportJson(draft)).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              });
            }}
          >
            {copied ? t("editor.copied") : t("editor.copyJson")}
          </Button>
        </div>
      </section>
    </div>
  );
}

export function ThemeEditor() {
  const { t } = useT(["theme", "common"]);
  const open = useThemeStore((s) => s.editorOpen);
  const editor = useThemeStore((s) => s.editor);
  const themes = useThemeStore((s) => s.themes);
  const busy = useThemeStore((s) => s.busy);
  const error = useThemeStore((s) => s.error);
  const store = useThemeStore.getState;
  const reduced = useVisualsStore((s) => s.visuals?.motion !== "full");
  const [tab, setTab] = useState<TabId>("colors");
  const [confirmClose, setConfirmClose] = useState(false);
  const [saved, setSaved] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) requestAnimationFrame(() => panelRef.current?.focus());
    else {
      setConfirmClose(false);
      setTab("colors");
    }
  }, [open]);

  const close = () => {
    if (editor?.dirty && !confirmClose) {
      setConfirmClose(true);
      return;
    }
    store().closeEditor();
  };

  const userIds = new Set(themes.filter((e) => !e.builtin).map((e) => e.theme.id));
  const draft = editor?.draft;
  const blocking = editor ? editor.issues.length > 0 : true;
  const asNew = editor ? savesAsNew(editor, userIds) : true;
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];
  const Body = current?.component;
  const onImportJson = (json: string): string[] | null => {
    const r = store().importIntoEditor(json);
    return r.ok ? null : r.issues;
  };

  return (
    <AnimatePresence>
      {open && editor && draft && (
        <motion.div
          key="theme-editor"
          ref={panelRef}
          tabIndex={-1}
          className="th-editor"
          role="dialog"
          aria-modal="false"
          aria-labelledby="th-editor-title"
          initial={reduced ? { opacity: 0 } : { opacity: 0, x: 40 }}
          animate={{ opacity: 1, x: 0 }}
          exit={reduced ? { opacity: 0 } : { opacity: 0, x: 40 }}
          transition={{ duration: reduced ? 0.01 : 0.28, ease: [0.2, 0.8, 0.2, 1] }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              close();
            }
          }}
        >
          <header className="th-editor__head">
            <ThemeMiniature theme={editor.preview} className="th-editor__mini" />
            <div className="th-editor__title">
              <span id="th-editor-title" className="th-editor__kicker">
                {t("theme:editor.title")}
              </span>
              <input
                className="th-editor__name"
                aria-label={t("theme:editor.name")}
                maxLength={80}
                value={draft.name}
                onChange={(e) => store().edit((th) => ({ ...th, name: e.target.value }))}
              />
              <span className="th-editor__sub">
                {asNew ? t("theme:editor.savesAsNew") : t("theme:editor.savesInPlace")}
                {editor.dirty && <Badge tone="warning">{t("theme:editor.unsaved")}</Badge>}
              </span>
            </div>
            <IconButton label={t("theme:editor.close")} icon={<X size={16} />} onClick={close} tooltip="left" />
          </header>

          <div className="th-editor__mode">
            <Segmented
              label={t("theme:editor.mode")}
              value={draft.mode}
              options={(["Dark", "Light"] as const).map((m) => ({ value: m, label: t(`theme:modes.${m}`) }))}
              onChange={(m) => store().edit((th) => ({ ...th, mode: m }))}
            />
            <span className="th-editor__live">
              <span className="th-editor__live-dot" /> {t("theme:editor.livePreview")}
            </span>
          </div>

          <nav className="th-editor__tabs" role="tablist" aria-label={t("theme:editor.sections")}>
            {TABS.map(({ id, icon: Icon }) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={tab === id}
                className={clsx("th-editor__tab", tab === id && "th-editor__tab--active")}
                onClick={() => setTab(id)}
                title={t(`theme:tabs.${id}`)}
              >
                <Icon size={14} />
                <span>{t(`theme:tabs.${id}`)}</span>
              </button>
            ))}
          </nav>

          <div className="th-editor__body" role="tabpanel">
            {Body ? <Body draft={draft} edit={(fn) => store().edit(fn)} /> : <JsonTab key={draft.id} draft={draft} onImport={onImportJson} />}
          </div>

          {(editor.issues.length > 0 || error) && (
            <div className="th-editor__issues" role="alert">
              {editor.issues.length > 0 && <b>{t("theme:editor.issues", { count: editor.issues.length })}</b>}
              <ul>
                {editor.issues.slice(0, 4).map((i) => (
                  <li key={i.path}>
                    <code>{i.path}</code> {i.message}
                  </li>
                ))}
                {error && <li>{error}</li>}
              </ul>
              {editor.issues.length > 0 && <span className="th-hint">{t("theme:editor.previewKept")}</span>}
            </div>
          )}

          {confirmClose && (
            <div className="th-editor__confirm" role="alertdialog" aria-label={t("theme:editor.discardTitle")}>
              <span>{t("theme:editor.discardText")}</span>
              <Button size="sm" variant="ghost" onClick={() => setConfirmClose(false)}>
                {t("theme:editor.keepEditing")}
              </Button>
              <Button size="sm" variant="danger" onClick={() => store().closeEditor()}>
                {t("theme:editor.discard")}
              </Button>
            </div>
          )}

          <footer className="th-editor__foot">
            <IconButton label={t("theme:editor.reset")} icon={<RotateCcw size={15} />} disabled={!editor.dirty} onClick={() => store().resetEditor()} tooltip="top" />
            <IconButton
              label={t("theme:import.button")}
              icon={<Upload size={15} />}
              onClick={() => fileRef.current?.click()}
              tooltip="top"
            />
            <IconButton
              label={t("theme:gallery.export")}
              icon={<Download size={15} />}
              onClick={() => downloadText(exportFileName(draft), store().exportJson(draft))}
              tooltip="top"
            />
            <input
              ref={fileRef}
              type="file"
              accept=".json,application/json"
              hidden
              onChange={(e) => {
                const f = e.target.files?.[0];
                e.target.value = "";
                if (f) void f.text().then((txt) => onImportJson(txt));
              }}
            />
            <span className="th-editor__spacer" />
            <Button variant="ghost" onClick={close}>
              {t("common:actions.cancel")}
            </Button>
            <Button
              variant="primary"
              leftIcon={<Save size={14} />}
              loading={busy}
              disabled={blocking || (!editor.dirty && !asNew)}
              onClick={() =>
                void store()
                  .saveEditor()
                  .then((ok) => {
                    if (!ok) return;
                    setSaved(true);
                    setTimeout(() => setSaved(false), 1600);
                  })
              }
            >
              {saved ? t("theme:editor.saved") : asNew ? t("theme:editor.saveAsNew") : t("theme:editor.save")}
            </Button>
          </footer>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
