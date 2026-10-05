import { Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import type { ReactNode } from "react";

import { useT } from "../../../i18n";

/** Quiet empty state inside a widget. */
export function WidgetEmpty({ icon, title, hint, badge }: { icon?: ReactNode; title: string; hint?: string; badge?: ReactNode }) {
  return (
    <div className="mc-empty">
      {icon && <span className="mc-empty__icon">{icon}</span>}
      <span className="mc-empty__title">{title}</span>
      {hint && <span className="mc-empty__hint">{hint}</span>}
      {badge}
    </div>
  );
}

/** Loading / error states shared by widgets. */
export function WidgetStatus({ loading, error }: { loading: boolean; error: string | null }) {
  const { t } = useT("common");
  if (error) return <div className="mc-error" role="alert">{error}</div>;
  if (loading) {
    return (
      <div className="mc-empty">
        <Spinner size={16} label={t("state.loading")} />
      </div>
    );
  }
  return null;
}

/** Big number with a caption. */
export function Stat({ value, label, tone, hint }: { value: ReactNode; label: string; tone?: string; hint?: ReactNode }) {
  return (
    <div className={clsx("mc-stat", tone && `mc-stat--${tone}`)}>
      <span className="mc-stat__value">{value}</span>
      <span className="mc-stat__label">{label}</span>
      {hint && <span className="mc-stat__hint">{hint}</span>}
    </div>
  );
}
