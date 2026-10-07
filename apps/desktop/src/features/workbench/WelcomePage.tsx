/**
 * VS Code-like Welcome page, shown in the main area while no project is
 * open: "Start" actions, the "Recent" list (click to open, × to remove from
 * the list) and a few keyboard tips.
 */
import "./workbench.css";

import { Kbd, Spinner } from "@orkestr/ui";
import { FileInput, FolderGit2, FolderOpen, FolderPlus, type LucideIcon, X } from "lucide-react";
import { useMemo } from "react";

import { Logo } from "../../components/Logo";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { chordSteps } from "../commands/keys";
import { openFileCommand, openFolderCommand, openRecent } from "./actions";
import { SHORTCUTS } from "./commands";
import { recentProjects, useWorkbenchStore } from "./store";

function Keys({ shortcut }: { shortcut: string }) {
  return (
    <span className="wb-keys">
      {chordSteps(shortcut).map((step, i) => (
        <span key={i} className="wb-keys__chord">
          {step.split("+").map((k, j) => (
            <Kbd key={j}>{k}</Kbd>
          ))}
        </span>
      ))}
    </span>
  );
}

interface StartAction {
  id: string;
  icon: LucideIcon;
  label: string;
  hint: string;
  shortcut?: string;
  run: () => void;
}

export function WelcomePage() {
  const { t } = useT("workbench");
  const projects = useAppStore((s) => s.projects);
  const loaded = useAppStore((s) => s.projectsLoaded);
  const hidden = useWorkbenchStore((s) => s.hiddenRecent);
  const hideRecent = useWorkbenchStore((s) => s.hideRecent);
  const recent = useMemo(() => recentProjects(projects, hidden), [projects, hidden]);

  const actions: StartAction[] = [
    {
      id: "new",
      icon: FolderPlus,
      label: t("welcome.newProject"),
      hint: t("welcome.newProjectHint"),
      run: () => useAppStore.getState().setCreateProjectOpen(true),
    },
    {
      id: "folder",
      icon: FolderOpen,
      label: t("welcome.openFolder"),
      hint: t("welcome.openFolderHint"),
      shortcut: SHORTCUTS.openFolder,
      run: () => void openFolderCommand(),
    },
    {
      id: "clone",
      icon: FolderGit2,
      label: t("welcome.clone"),
      hint: t("welcome.cloneHint"),
      run: () => useWorkbenchStore.getState().setCloneOpen(true),
    },
    {
      id: "file",
      icon: FileInput,
      label: t("welcome.openFile"),
      hint: t("welcome.openFileHint"),
      shortcut: SHORTCUTS.openFile,
      run: () => void openFileCommand(),
    },
  ];

  return (
    <section className="wb-welcome" aria-labelledby="wb-welcome-title">
      <div className="wb-welcome__inner">
        <header className="wb-welcome__header">
          <Logo size={44} />
          <div>
            <h1 id="wb-welcome-title" className="wb-welcome__title">
              {t("welcome.title")}
            </h1>
            <p className="wb-welcome__subtitle">{t("welcome.subtitle")}</p>
          </div>
        </header>

        <div className="wb-welcome__grid">
          <div className="wb-welcome__col">
            <h2 className="wb-welcome__heading">{t("welcome.start")}</h2>
            <ul className="wb-welcome__actions">
              {actions.map((a) => (
                <li key={a.id}>
                  <button type="button" className="wb-welcome__action" onClick={a.run}>
                    <span className="wb-welcome__action-icon">
                      <a.icon size={17} />
                    </span>
                    <span className="wb-welcome__action-text">
                      <span className="wb-welcome__action-label">{a.label}</span>
                      <span className="wb-welcome__action-hint">{a.hint}</span>
                    </span>
                    {a.shortcut && <Keys shortcut={a.shortcut} />}
                  </button>
                </li>
              ))}
            </ul>

            <h2 className="wb-welcome__heading">{t("welcome.help")}</h2>
            <ul className="wb-welcome__tips">
              <li>
                <Keys shortcut="Ctrl+K" />
                <span>{t("welcome.hintPalette")}</span>
              </li>
              <li>
                <Keys shortcut={SHORTCUTS.openFolder} />
                <span>{t("welcome.hintOpenFolder")}</span>
              </li>
              <li>
                <Keys shortcut={SHORTCUTS.openFile} />
                <span>{t("welcome.hintOpenFile")}</span>
              </li>
              <li>
                <Keys shortcut={SHORTCUTS.newFile} />
                <span>{t("welcome.hintNewFile")}</span>
              </li>
            </ul>
          </div>

          <div className="wb-welcome__col">
            <h2 className="wb-welcome__heading">{t("welcome.recent")}</h2>
            {!loaded ? (
              <div className="wb-welcome__empty">
                <Spinner size={14} /> {t("welcome.loading")}
              </div>
            ) : recent.length === 0 ? (
              <p className="wb-welcome__empty">{t("welcome.recentEmpty")}</p>
            ) : (
              <ul className="wb-welcome__recent">
                {recent.map((p) => (
                  <li key={p.id} className="wb-recent">
                    <button
                      type="button"
                      className="wb-recent__open"
                      aria-label={t("welcome.open", { name: p.name })}
                      title={p.rootPath ?? p.name}
                      onClick={() => openRecent(p.id)}
                    >
                      <span className="wb-recent__dot" style={{ background: p.color ?? undefined }} />
                      <span className="wb-recent__name">{p.name}</span>
                      <span className="wb-recent__path">{p.rootPath ?? t("welcome.noFolder")}</span>
                    </button>
                    <button
                      type="button"
                      className="wb-recent__remove"
                      aria-label={t("welcome.remove", { name: p.name })}
                      title={t("welcome.remove", { name: p.name })}
                      onClick={() => hideRecent(p)}
                    >
                      <X size={13} />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
