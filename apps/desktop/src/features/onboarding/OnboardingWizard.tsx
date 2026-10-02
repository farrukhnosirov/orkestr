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

import { Logo } from "../../components/Logo";
import { getClient, openProject } from "../../core/client";
import { useAppStore } from "../../store/app";
import { callOptional } from "../settings/optionalRpc";
import { SystemChecksList } from "../settings/sections/SystemChecksList";
import { useSettingsStore } from "../settings/settingsStore";
import { useSystemStore } from "../settings/systemStore";
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

const STEP_ICONS = [UserRound, HardDrive, KeyRound, Bot, ShieldCheck, FolderPlus, GitBranch, SquareTerminal, Mic, Wifi, Cloud, Smartphone];

export const PERMISSION_MODES: ReadonlyArray<{ mode: ApprovalMode; title: string; body: string; badge?: string }> = [
  {
    mode: "Ask",
    title: "Ask",
    body: "Agents ask before every change, command or network call. Maximum control — recommended to start.",
    badge: "Recommended",
  },
  {
    mode: "SemiAuto",
    title: "Semi-Auto",
    body: "Routine work inside the project sandbox runs on its own; anything risky (deletes, installs, pushes) asks first.",
  },
  {
    mode: "FullAuto",
    title: "Full Auto",
    body: "Everything your policies allow runs without asking. Dangerous actions (force-push, DROP, prod deploys) still always ask.",
  },
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
  return (
    <div className="onb-later">
      <div className="onb-later__icon">{icon}</div>
      <Badge tone="violet">Phase {phase}</Badge>
      <h3 className="onb-later__title">{title}</h3>
      <p className="onb-later__body">{body}</p>
      <p className="onb-later__note">Nothing to set up yet — this step unlocks automatically when the feature ships.</p>
    </div>
  );
}

function ChecksFor({ ids }: { ids: readonly string[] }) {
  const { checks, checksLoading, checksError, runChecks } = useSystemStore();
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
          <Spinner size={16} /> Checking your system…
        </div>
      ) : (
        <SystemChecksList checks={subset} />
      )}
      <Button size="sm" variant="ghost" leftIcon={<RefreshCw size={13} />} loading={checksLoading} onClick={() => void runChecks()}>
        Check again
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
    else setError(useSettingsStore.getState().error ?? "Could not save your setup.");
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
      if (!ok) return setError(useSettingsStore.getState().error ?? "Could not save your profile.");
    } else if (current.id === "storage") {
      setBusy(true);
      const ok = await updateSettings({ storageMode: "LocalOnly" });
      setBusy(false);
      if (!ok) return setError(useSettingsStore.getState().error ?? "Could not save the storage mode.");
    } else if (current.id === "permissions") {
      setBusy(true);
      try {
        const r = await callOptional("permissions.setMode", { mode });
        setModeNote(r.available ? null : "Saved as your default; the permission engine applies it when it is available.");
      } catch (e) {
        setBusy(false);
        return setError(e instanceof Error ? e.message : String(e));
      }
      const ok = await updateSettings({ defaultApprovalMode: mode });
      setBusy(false);
      if (!ok) return setError(useSettingsStore.getState().error ?? "Could not save the permission mode.");
    }
    if (canFinishAt(step) && step === ONBOARDING_STEPS.length - 1) return finish();
    go(step + 1);
  };

  const createProject = async () => {
    const trimmed = projectName.trim();
    const pathProblem = validateFolderPath(projectPath);
    if (!trimmed) return setProjectError("Give the project a name.");
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
      setProjectError(e instanceof OrkestrError ? e.message : "Could not create the project.");
    } finally {
      setBusy(false);
    }
  };

  let body: ReactNode;
  switch (current.id) {
    case "profile":
      body = (
        <>
          <p className="onb-lead">Orkestr runs locally. Your profile never leaves this machine — no sign-up, no account.</p>
          <Input
            label="What should agents call you?"
            placeholder="e.g. Ada"
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
        <div role="radiogroup" aria-label="Storage mode" className="onb-choices">
          <Choice
            selected
            onSelect={() => {}}
            icon={<HardDrive size={18} />}
            title="Local only"
            body="Projects, history, memory and secrets stay on this computer. You can add sync later."
            badge={<Badge tone="success">Available</Badge>}
          />
          <Choice
            selected={false}
            disabled
            onSelect={() => {}}
            icon={<Server size={18} />}
            title="Self-hosted"
            body="Sync through your own Orkestr server."
            badge={<Badge tone="violet">Phase 4</Badge>}
          />
          <Choice
            selected={false}
            disabled
            onSelect={() => {}}
            icon={<Cloud size={18} />}
            title="Orkestr Cloud"
            body="End-to-end encrypted sync across your devices."
            badge={<Badge tone="violet">Phase 4</Badge>}
          />
        </div>
      );
      break;
    case "providers":
      body = (
        <div className="onb-slot">
          {/* INTEGRATOR: embed <ProviderSetup/> */}
          <KeyRound size={22} />
          <h3>Connect AI providers</h3>
          <p>
            Use your own API keys or the official CLIs you are already signed in to (Claude Code, Codex). Keys are stored in
            the OS credential store, never in plain text.
          </p>
          <p className="onb-slot__note">Provider setup is not part of this build yet — you can skip this step and connect providers later.</p>
        </div>
      );
      break;
    case "manager":
      body = (
        <div className="onb-slot">
          {/* INTEGRATOR: embed <ManagerModelPicker/> */}
          <Bot size={22} />
          <h3>Choose the Manager model</h3>
          <p>The Manager plans work, splits it into tasks and delegates to agents. A strong reasoning model works best.</p>
          <p className="onb-slot__note">Available once a provider is connected — skip for now.</p>
        </div>
      );
      break;
    case "permissions":
      body = (
        <>
          <div role="radiogroup" aria-label="Permission mode" className="onb-choices">
            {PERMISSION_MODES.map((p) => (
              <Choice
                key={p.mode}
                selected={mode === p.mode}
                onSelect={() => setMode(p.mode)}
                icon={<ShieldCheck size={18} />}
                title={p.title}
                body={p.body}
                badge={p.badge ? <Badge tone="accent">{p.badge}</Badge> : undefined}
              />
            ))}
          </div>
          <p className="onb-hint">You can change this per project at any time. The emergency stop always works.</p>
          {modeNote && <p className="onb-hint">{modeNote}</p>}
        </>
      );
      break;
    case "project":
      body = createdProject ? (
        <div className="onb-success">
          <Check size={20} /> <strong>{createdProject}</strong> is ready and open.
        </div>
      ) : (
        <>
          <p className="onb-lead">Point Orkestr at an existing folder (a git repository works best) or create an empty project.</p>
          <Input label="Project name" placeholder="e.g. Payroll Platform" value={projectName} onChange={(e) => setProjectName(e.target.value)} />
          <Input
            label="Folder (optional)"
            placeholder="C:\Code\my-app"
            value={projectPath}
            hint="Absolute path of an existing folder. Leave empty to start without files."
            onChange={(e) => setProjectPath(e.target.value)}
            error={projectError ?? undefined}
          />
          <div>
            <Button variant="primary" leftIcon={<FolderPlus size={14} />} loading={busy} disabled={!connected} onClick={() => void createProject()}>
              Create project
            </Button>
          </div>
        </>
      );
      break;
    case "git":
      body = (
        <>
          <p className="onb-lead">Agents work on git branches and worktrees so every change can be reviewed and rolled back.</p>
          <ChecksFor ids={["git"]} />
        </>
      );
      break;
    case "terminal":
      body = (
        <>
          <p className="onb-lead">Agents run commands in real terminals. Here is what Orkestr found on this machine.</p>
          <ChecksFor ids={["shell", "dataDir", "webview", "keyring"]} />
        </>
      );
      break;
    case "voice":
      body = (
        <LaterCard
          icon={<Mic size={26} />}
          phase={current.phase}
          title="Voice control"
          body="Wake word, push-to-talk and spoken notifications for the Manager — processed locally where possible."
        />
      );
      break;
    case "remote":
      body = (
        <LaterCard
          icon={<Wifi size={26} />}
          phase={current.phase}
          title="Remote access"
          body="Reach this machine's agents securely through the Orkestr gateway — the core never opens ports."
        />
      );
      break;
    case "sync":
      body = (
        <LaterCard
          icon={<Cloud size={26} />}
          phase={current.phase}
          title="Cloud sync"
          body="Encrypted sync of projects, memory and settings between your devices."
        />
      );
      break;
    case "mobile":
      body = (
        <LaterCard
          icon={<Smartphone size={26} />}
          phase={current.phase}
          title="Mobile pairing"
          body="Approve actions, watch progress and stop agents from your phone."
        />
      );
      break;
  }

  const isLast = step === ONBOARDING_STEPS.length - 1;
  const later = current.phase > CURRENT_PHASE;

  return (
    <div className="overlay onb-overlay" role="presentation">
      <div className="onb" role="dialog" aria-modal="true" aria-labelledby="onb-title" onKeyDown={(e) => e.key === "Escape" && closeWizard()}>
        <aside className="onb-steps" aria-label="Setup steps">
          <div className="onb-steps__brand">
            <Logo size={26} /> <span>Welcome to Orkestr</span>
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
                    {s.phase > CURRENT_PHASE && <span className="onb-step__phase">P{s.phase}</span>}
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
                Step {step + 1} of {ONBOARDING_STEPS.length}
                {current.optional && !later && <Badge>Optional</Badge>}
                {later && <Badge tone="violet">Coming in Phase {current.phase}</Badge>}
              </div>
              <h2 id="onb-title" className="onb-main__title">
                {current.title}
              </h2>
              <p className="onb-main__subtitle">{current.subtitle}</p>
            </div>
            <button type="button" className="dialog__close" aria-label="Close setup" onClick={closeWizard}>
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
              Skip setup
            </Button>
            <div className="onb-main__spacer" />
            <Button variant="secondary" onClick={() => go(step - 1)} disabled={step === 0 || busy}>
              Back
            </Button>
            {canFinishAt(step) && !isLast && (
              <Button variant="secondary" onClick={() => void finish()} loading={busy}>
                Finish
              </Button>
            )}
            <Button variant="primary" onClick={() => void next()} loading={busy} disabled={!connected && !later}>
              {isLast ? "Finish setup" : current.optional && !later && current.id !== "permissions" ? "Next / Skip" : "Next"}
            </Button>
          </footer>
        </section>
      </div>
    </div>
  );
}
