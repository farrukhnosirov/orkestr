import { Button, EmptyState, Input } from "@orkestr/ui";
import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";

import { type RemoteClient, errorCode } from "../remote";

/** Minimal text editor over `files.read` / `files.write` (optimistic concurrency via hash). */
export function EditorPanel({ client, projectId }: { client: RemoteClient; projectId: string | null }) {
  const { t } = useTranslation();
  const [path, setPath] = useState("");
  const [file, setFile] = useState<{ path: string; hash: string; readonly: boolean } | null>(null);
  const [content, setContent] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!projectId) return <EmptyState title={t("editor.pickProject")} />;

  const load = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setStatus(null);
    try {
      const r = await client.call("files.read", { projectId, path: path.trim() });
      if (r.binary || r.content === null) {
        setFile(null);
        setStatus(t("editor.binary"));
      } else {
        setFile({ path: r.path, hash: r.hash, readonly: r.readonly });
        setContent(r.content);
      }
    } catch (err) {
      setStatus(t(`errors.${errorCode(err)}`, { defaultValue: t("errors.internal") }));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!file) return;
    setBusy(true);
    try {
      const r = await client.call("files.write", { projectId, path: file.path, content, expectedHash: file.hash });
      setFile({ ...file, hash: r.hash });
      setStatus(t("editor.saved"));
    } catch (err) {
      setStatus(t(`errors.${errorCode(err)}`, { defaultValue: t("errors.internal") }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-label={t("tabs.editor")} className="web-stack">
      <form className="web-toolbar" onSubmit={(e) => void load(e)}>
        <Input aria-label={t("editor.path")} placeholder={t("editor.path")} value={path} onChange={(e) => setPath(e.target.value)} />
        <Button type="submit" disabled={busy || !path.trim()}>
          {t("editor.open")}
        </Button>
      </form>
      {file && (
        <>
          <textarea
            aria-label={t("editor.content")}
            className="web-editor"
            spellCheck={false}
            readOnly={file.readonly}
            value={content}
            onChange={(e) => setContent(e.target.value)}
          />
          <Button variant="primary" loading={busy} disabled={file.readonly} onClick={() => void save()}>
            {t("editor.save")}
          </Button>
        </>
      )}
      {status && <p role="status">{status}</p>}
    </section>
  );
}
