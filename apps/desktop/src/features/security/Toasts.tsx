import { ShieldAlert, X } from "lucide-react";
import { useEffect } from "react";

import { useT } from "../../i18n";
import { type Toast, useSecurityStore } from "./store";

const TOAST_MS = 8000;

function ToastItem({ toast }: { toast: Toast }) {
  const dismiss = useSecurityStore((s) => s.dismissToast);
  const openInbox = useSecurityStore((s) => s.setInboxOpen);
  const { t } = useT("security");
  useEffect(() => {
    const timer = window.setTimeout(() => dismiss(toast.id), TOAST_MS);
    return () => window.clearTimeout(timer);
  }, [toast.id, dismiss]);
  return (
    <div className={`sec-toast sec-toast--${toast.tone}`} role="status">
      <ShieldAlert size={16} aria-hidden="true" />
      <div className="sec-toast__text">
        <strong>{toast.title}</strong>
        <span>{toast.body}</span>
        {toast.approvalId && (
          <button
            type="button"
            className="sec-link"
            onClick={() => {
              openInbox(true);
              dismiss(toast.id);
            }}
          >
            {t("toast.review")}
          </button>
        )}
      </div>
      <button type="button" className="sec-toast__close" aria-label={t("toast.dismiss")} onClick={() => dismiss(toast.id)}>
        <X size={14} />
      </button>
    </div>
  );
}

/** Toast stack (approval requests, emergency results). */
export function SecurityToasts() {
  const toasts = useSecurityStore((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="sec-toasts" aria-live="polite">
      {toasts.map((item) => (
        <ToastItem key={item.id} toast={item} />
      ))}
    </div>
  );
}
