import { OrkestrError } from "@orkestr/client";
import type { ApprovalMode, SystemCheck } from "@orkestr/protocol";
import { Badge, Button, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  Bot,
  Check,
  Cloud,
  FolderPlus,
  GitBranch,
  HardDrive,
  KeyRound,
  Mic,
  RefreshCw,
  Server,
  ShieldCheck,
  Smartphone,
  SquareTerminal,
  UserRound,
  Wifi,
  X,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Trans } from "react-i18next";

import { LanguageSelect } from "../../components/LanguageSelect";
import { Logo } from "../../components/Logo";
import { PathInput } from "../../components/PathInput";
import { t, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { getClient, openProject } from "../../core/client";
import { useAppStore } from "../../store/app";
import { ClaudeEngineCard } from "../claudeEngine/ClaudeEngineCard";
import { ManagerSettings, ProviderSetup } from "../providers";
import { SystemChecksList } from "../settings/sections/SystemChecksList";
import { useSettingsStore } from "../settings/settingsStore";
import { useSystemStore } from "../settings/systemStore";
import { VoiceQuickSetup } from "../voice/settings/VoiceQuickSetup";
import { hostBasename } from "../workbench/hostPaths";
import "../settings/settings.css";
import "./onboarding.css";
import { useOnboardingStore } from "./onboardingStore";
import {
  canFinishAt,
  clampStep,
  CURRENT_PHASE,
  ONBOARDING_STEPS,
  stepState,
  validateFolderPath,
  validateProfileName,
} from "./steps";
import { Overlay } from "../../components/Overlay";

const STEP_ICONS = [UserRound, HardDrive, KeyRound, Bot, ShieldCheck, FolderPlus, GitBranch, SquareTerminal, Mic, Wifi, Cloud, Smartphone];

/** Permission modes offered on the permissions step (texts are localized at render time). */
export const PERMISSION_MODES: ReadonlyArray<{ mode: ApprovalMode; recommended?: boolean }> = [
  { mode: "Ask", recommended: true },
  { mode: "SemiAuto" },
  { mode: "FullAuto" },
];

function Choice({
  selected,
  disabled,
  onSelect,
  icon,
  title,
  body,
  badge,
}: {
  selected: boolean;
  disabled?: boolean;
  onSelect: () => void;
  icon: ReactNode;
  title: string;
  body: string;
  badge?: ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      aria-disabled={disabled || undefined}
      disabled={disabled}
      className={clsx("onb-choice", selected && "onb-choice--active", disabled && "onb-choice--disabled")}
      onClick={onSelect}
    >
      <span className="onb-choice__icon">{icon}</span>
      <span className="onb-choice__text">
        <span className="onb-choice__title">
          {title} {badge}
        </span>
        <span className="onb-choice__body">{body}</span>
      </span>
      {selected && <Check size={16} className="onb-choice__check" />}
    </button>
  );
}

function LaterCard({ icon, phase, title, body }: { icon: ReactNode; phase: number; title: string; body: string }) {
  const { t } = useT(["onboarding", "common"]);
  return (
    <div className="onb-later">
      <div className="onb-later__icon">{icon}</div>
      <Badge tone="violet">{t("common:phase", { phase })}</Badge>
      <h3 className="onb-later__title">{title}</h3>
      <p className="onb-later__body">{body}</p>
      <p className="onb-later__note">{t("wizard.laterNote")}</p>
    </div>
  );
}

function ChecksFor({ ids }: { ids: readonly string[] }) {
  const { checks, checksLoading, checksError, runChecks } = useSystemStore();
  const { t } = useT("onboarding");
  useEffect(() => {
    if (!useSystemStore.getState().checks) void runChecks();
  }, [runChecks]);
  const subset: SystemCheck[] = checks ? checks.checks.filter((c) => ids.includes(c.id)) : [];
  return (
    <div className="onb-checks">
      {checksError && !checks ? (
        <div className="set-alert" role="alert">
          {checksError}
        </div>
      ) : !checks ? (
        <div className="set-loading">
          <Spinner size={16} /> {t("wizard.checking")}
        </div>
      ) : (
        <SystemChecksList checks={subset} />
      )}
      <Button size="sm" variant="ghost" leftIcon={<RefreshCw size={13} />} loading={checksLoading} onClick={() => void runChecks()}>
        {t("wizard.checkAgain")}
      </Button>
    </div>
  );
}

/** First-run setup wizard (§55). Re-open from Settings → General or the command palette. */
export function OnboardingWizard() {
  const open = useOnboardingStore((s) => s.open);
  const step = useOnboardingStore((s) => s.step);
  const setStep = useOnboardingStore((s) => s.setStep);
  const closeWizard = useOnboardingStore((s) => s.closeWizard);
  const settings = useSettingsStore((s) => s.settings);
  const updateSettings = useSettingsStore((s) => s.update);
  const connected = useAppStore((s) => s.connection.state === "open");

  const [visited, setVisited] = useState<Set<number>>(() => new Set());
  const [name, setName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [mode, setMode] = useState<ApprovalMode>("Ask");
  const [modeNote, setModeNote] = useState<string | null>(null);
  const [projectName, setProjectName] = useState("");
  const [projectPath, setProjectPath] = useState("");
  const [projectError, setProjectError] = useState<string | null>(null);
  const [createdProject, setCreatedProject] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const { t: tr } = useT(["onboarding", "common"]); // also re-renders on language change

  useEffect(() => {
    if (!open) return;
    setVisited(new Set());
    setError(null);
    setCreatedProject(null);
    if (settings) {
      setName(settings.profileName);
      setMode(settings.defaultApprovalMode);
    }
    // Initialise only when the wizard opens.
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (open) requestAnimationFrame(() => bodyRef.current?.querySelector<HTMLElement>("input, button, [tabindex]")?.focus());
  }, [open, step]);

  const current = ONBOARDING_STEPS[step];
  const stepDone = useMemo(() => new Set(visited), [visited]);
  if (!open || !current) return null;

  const go = (next: number) => {
    setVisited((v) => new Set(v).add(step));
    setError(null);
    setStep(clampStep(next));
  };

  const finish = async () => {
    setBusy(true);
    const ok = await updateSettings({ onboardingCompleted: true });
    setBusy(false);
    if (ok) closeWizard();
    else setError(useSettingsStore.getState().error ?? t("onboarding:errors.saveSetup"));
  };

  /** Persists the current step (if it has data) and advances. */
  const next = async () => {
    setError(null);
    if (current.id === "profile") {
      const problem = validateProfileName(name);
      setNameError(problem);
      if (problem) return;
      setBusy(true);
      const ok = await updateSettings({ profileName: name.trim() });
      setBusy(false);
      if (!ok) return setError(useSettingsStore.getState().error ?? t("onboarding:errors.saveProfile"));
    } else if (current.id === "storage") {
      setBusy(true);
      const ok = await updateSettings({ storageMode: "LocalOnly" });
      setBusy(false);
      if (!ok) return setError(useSettingsStore.getState().error ?? t("onboarding:errors.saveStorage"));
    } else if (current.id === "permissions") {
      setBusy(true);
      try {
        // The default applies to new projects; the open project switches now.
        const projectId = useAppStore.getState().activeProjectId;
        if (projectId) await getClient().call("permissions.setMode", { projectId, mode });
        setModeNote(projectId ? null : t("onboarding:permissions.savedAsDefault"));
      } catch (e) {
        setBusy(false);
        return setError(describeError(e));
      }
      const ok = await updateSettings({ defaultApprovalMode: mode });
      setBusy(false);
      if (!ok) return setError(useSettingsStore.getState().error ?? t("onboarding:errors.saveMode"));
    }
    if (canFinishAt(step) && step === ONBOARDING_STEPS.length - 1) return finish();
    go(step + 1);
  };

  const createProject = async () => {
    const trimmed = projectName.trim();
    const pathProblem = validateFolderPath(projectPath);
    if (!trimmed) return setProjectError(t("onboarding:validation.projectNameRequired"));
    if (pathProblem) return setProjectError(pathProblem);
    setProjectError(null);
    setBusy(true);
    try {
      const params: { name: string; rootPath?: string } = { name: trimmed };
      if (projectPath.trim()) params.rootPath = projectPath.trim();
      const { project } = await getClient().call("projects.create", params);
      useAppStore.getState().upsertProject(project);
      await openProject(project.id);
      setCreatedProject(project.name);
    } catch (e) {
      setProjectError(e instanceof OrkestrError ? describeError(e) : t("onboarding:errors.createProject"));
    } finally {
      setBusy(false);
    }
  };

  let body: ReactNode;
  switch (current.id) {
    case "profile":
      body = (
        <>
          <div className="ok-field">
            <label className="ok-field__label" htmlFor="onb-language">
              {t("common:language.label")}
            </label>
            <LanguageSelect id="onb-language" />
          </div>
          <p className="onb-lead">{t("onboarding:profile.lead")}</p>
          <Input
            label={t("onboarding:profile.nameLabel")}
            placeholder={t("onboarding:profile.namePlaceholder")}
            value={name}
            maxLength={80}
            error={nameError ?? undefined}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void next()}
          />
        </>
      );
      break;
    case "storage":
      body = (
        <div role="radiogroup" aria-label={t("onboarding:storage.label")} className="onb-choices">
          <Choice
            selected
            onSelect={() => {}}
            icon={<HardDrive size={18} />}
            title={t("onboarding:storage.local.title")}
            body={t("onboarding:storage.local.body")}
            badge={<Badge tone="success">{t("onboarding:storage.available")}</Badge>}
          />
          <Choice
            selected={false}
            disabled
            onSelect={() => {}}
            icon={<Server size={18} />}
            title={t("onboarding:storage.selfHosted.title")}
            body={t("onboarding:storage.selfHosted.body")}
            badge={<Badge tone="violet">{t("common:phase", { phase: 4 })}</Badge>}
          />
          <Choice
            selected={false}
            disabled
            onSelect={() => {}}
            icon={<Cloud size={18} />}
            title={t("onboarding:storage.cloud.title")}
            body={t("onboarding:storage.cloud.body")}
            badge={<Badge tone="violet">{t("common:phase", { phase: 4 })}</Badge>}
          />
        </div>
      );
      break;
    case "providers":
      body = (
        <div className="onb-embed">
          <p className="onb-hint">
            <KeyRound size={14} /> {t("onboarding:providers.hint")}
          </p>
          <ClaudeEngineCard />
          <ProviderSetup compact />
        </div>
      );
      break;
    case "manager":
      body = (
        <div className="onb-embed">
          <p className="onb-hint">
            <Bot size={14} /> {t("onboarding:manager.hint")}
          </p>
          <ManagerSettings />
        </div>
      );
      break;
    case "permissions":
      body = (
        <>
          <div role="radiogroup" aria-label={t("onboarding:permissions.label")} className="onb-choices">
            {PERMISSION_MODES.map((p) => (
              <Choice
                key={p.mode}
                selected={mode === p.mode}
                onSelect={() => setMode(p.mode)}
                icon={<ShieldCheck size={18} />}
                title={t(`common:status.approvalMode.${p.mode}`)}
                body={t(`onboarding:modes.${p.mode}`)}
                badge={p.recommended ? <Badge tone="accent">{t("onboarding:modes.recommended")}</Badge> : undefined}
              />
            ))}
          </div>
          <p className="onb-hint">{t("onboarding:permissions.hint")}</p>
          {modeNote && <p className="onb-hint">{modeNote}</p>}
        </>
      );
      break;
    case "project":
      body = createdProject ? (
        <div className="onb-success">
          <Check size={20} />{" "}
          <span>
            <Trans t={tr} i18nKey="onboarding:project.created" values={{ name: createdProject }} components={{ strong: <strong /> }} />
          </span>
        </div>
      ) : (
        <>
          <p className="onb-lead">{t("onboarding:project.lead")}</p>
          <Input
            label={t("onboarding:project.nameLabel")}
            placeholder={t("onboarding:project.namePlaceholder")}
            value={projectName} onChange={(e) => setProjectName(e.target.value)} />
          <PathInput
            label={t("onboarding:project.folderLabel")}
            placeholder={t("onboarding:project.folderPlaceholder")}
            value={projectPath}
            hint={t("onboarding:project.folderHint")}
            onPathChange={setProjectPath}
            mapPicked={(folder) => {
              // Like "Open folder": a picked folder names a still unnamed project.
              if (!projectName.trim()) setProjectName(hostBasename(folder));
              return folder;
            }}
            error={projectError ?? undefined}
          />
          <div>
            <Button variant="primary" leftIcon={<FolderPlus size={14} />} loading={busy} disabled={!connected} onClick={() => void createProject()}>
              {t("onboarding:project.create")}
            </Button>
          </div>
        </>
      );
      break;
    case "git":
      body = (
        <>
          <p className="onb-lead">{t("onboarding:git.lead")}</p>
          <ChecksFor ids={["git"]} />
        </>
      );
      break;
    case "terminal":
      body = (
        <>
          <p className="onb-lead">{t("onboarding:terminal.lead")}</p>
          <ChecksFor ids={["shell", "dataDir", "webview", "keyring"]} />
        </>
      );
      break;
    case "voice":
      body = (
        <div className="onb-embed">
          <p className="onb-hint">
            <Mic size={14} /> {t("onboarding:voice.lead")}
          </p>
          <VoiceQuickSetup />
          <p className="onb-hint">{t("onboarding:voice.more")}</p>
        </div>
      );
      break;
    case "remote":
      body = (
        <LaterCard
          icon={<Wifi size={26} />}
          phase={current.phase}
          title={t("onboarding:later.remote.title")}
          body={t("onboarding:later.remote.body")}
        />
      );
      break;
    case "sync":
      body = (
        <LaterCard
          icon={<Cloud size={26} />}
          phase={current.phase}
          title={t("onboarding:later.sync.title")}
          body={t("onboarding:later.sync.body")}
        />
      );
      break;
    case "mobile":
      body = (
        <LaterCard
          icon={<Smartphone size={26} />}
          phase={current.phase}
          title={t("onboarding:later.mobile.title")}
          body={t("onboarding:later.mobile.body")}
        />
      );
      break;
  }

  const isLast = step === ONBOARDING_STEPS.length - 1;
  const later = current.phase > CURRENT_PHASE;

  return (
    <Overlay className="onb-overlay">
      <div className="onb" role="dialog" aria-modal="true" aria-labelledby="onb-title" onKeyDown={(e) => e.key === "Escape" && closeWizard()}>
        <aside className="onb-steps" aria-label={t("onboarding:wizard.stepsLabel")}>
          <div className="onb-steps__brand">
            <Logo size={26} /> <span>{t("onboarding:wizard.welcome")}</span>
          </div>
          <ol className="onb-steps__list">
            {ONBOARDING_STEPS.map((s, i) => {
              const state = stepState(i, step, stepDone);
              const Icon = STEP_ICONS[i] ?? Check;
              return (
                <li key={s.id}>
                  <button
                    type="button"
                    className={clsx("onb-step", `onb-step--${state}`)}
                    aria-current={state === "current" ? "step" : undefined}
                    onClick={() => go(i)}
                  >
                    <span className="onb-step__dot">{state === "done" ? <Check size={12} /> : <Icon size={12} />}</span>
                    <span className="onb-step__label">{s.title}</span>
                    {s.phase > CURRENT_PHASE && (
                      <span className="onb-step__phase" title={t("onboarding:wizard.comingIn", { phase: s.phase })}>
                        {t("onboarding:wizard.phaseShort", { phase: s.phase })}
                      </span>
                    )}
                  </button>
                </li>
              );
            })}
          </ol>
          <div className="onb-progress" aria-hidden="true">
            <div className="onb-progress__fill" style={{ width: `${((step + 1) / ONBOARDING_STEPS.length) * 100}%` }} />
          </div>
        </aside>
        <section className="onb-main">
          <header className="onb-main__head">
            <div>
              <div className="onb-main__count">
                {t("onboarding:wizard.stepCount", { current: step + 1, total: ONBOARDING_STEPS.length })}
                {current.optional && !later && <Badge>{t("common:state.optional")}</Badge>}
                {later && <Badge tone="violet">{t("onboarding:wizard.comingIn", { phase: current.phase })}</Badge>}
              </div>
              <h2 id="onb-title" className="onb-main__title">
                {current.title}
              </h2>
              <p className="onb-main__subtitle">{current.subtitle}</p>
            </div>
            <button type="button" className="dialog__close" aria-label={t("onboarding:wizard.close")} onClick={closeWizard}>
              <X size={16} />
            </button>
          </header>
          <div className="onb-main__body" ref={bodyRef}>
            {body}
            {error && (
              <div className="set-alert" role="alert">
                {error}
              </div>
            )}
          </div>
          <footer className="onb-main__foot">
            <Button variant="ghost" onClick={() => void finish()} disabled={busy}>
              {t("onboarding:wizard.skipSetup")}
            </Button>
            <div className="onb-main__spacer" />
            <Button variant="secondary" onClick={() => go(step - 1)} disabled={step === 0 || busy}>
              {t("common:actions.back")}
            </Button>
            {canFinishAt(step) && !isLast && (
              <Button variant="secondary" onClick={() => void finish()} loading={busy}>
                {t("common:actions.finish")}
              </Button>
            )}
            <Button variant="primary" onClick={() => void next()} loading={busy} disabled={!connected && !later}>
              {isLast
                ? t("onboarding:wizard.finishSetup")
                : current.optional && !later && current.id !== "permissions"
                  ? t("onboarding:wizard.nextOrSkip")
                  : t("common:actions.next")}
            </Button>
          </footer>
        </section>
      </div>
    </Overlay>
  );
}
