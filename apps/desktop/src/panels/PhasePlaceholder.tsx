import { Badge, EmptyState } from "@orkestr/ui";
import { Trans } from "react-i18next";

import { useT } from "../i18n";
import placeholdersEn from "../locales/en/placeholders.json";
import { useActiveProject } from "../store/app";
import type { PanelIcon } from "./types";

/** Panels that are placeholders until a later phase (keys of the `placeholders` namespace). */
export type PlaceholderPanel = keyof typeof placeholdersEn;

export interface PhasePlaceholderProps {
  icon: PanelIcon;
  panel: PlaceholderPanel;
  phase: number;
}

/** Honest "not built yet" state for panels that arrive in a later phase. */
export function PhasePlaceholder({ icon: Icon, panel, phase }: PhasePlaceholderProps) {
  const project = useActiveProject();
  const { t } = useT(["placeholders", "shell"]);
  const title = t(`shell:panels.${panel}.title`);
  const features = Object.keys(placeholdersEn[panel].features);
  return (
    <div className="phase-placeholder">
      <EmptyState
        icon={<Icon size={28} strokeWidth={1.6} />}
        badge={
          <Badge tone="violet" className="phase-placeholder__badge">
            {t("shell:phasePlaceholder.badge", { phase })}
          </Badge>
        }
        title={title}
        description={
          <>
            {t(`placeholders:${panel}.summary`)}
            {project && (
              <span className="phase-placeholder__project">
                <Trans
                  t={t}
                  i18nKey="shell:phasePlaceholder.project"
                  values={{ name: project.name }}
                  components={{ strong: <strong /> }}
                />
              </span>
            )}
          </>
        }
      />
      <ul className="phase-placeholder__features" aria-label={t("shell:phasePlaceholder.features", { title })}>
        {features.map((f) => (
          <li key={f}>{t(`placeholders:${panel}.features.${f}` as "placeholders:browser.summary")}</li>
        ))}
      </ul>
    </div>
  );
}
