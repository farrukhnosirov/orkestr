/**
 * Text field for a filesystem path with a "Browse…" button that opens the
 * native folder/file dialog (desktop shell only — in a browser the button is
 * hidden and the path is typed). The field stays editable either way.
 */
import "../features/workbench/workbench.css";

import { Button, IconButton, Input, type InputProps } from "@orkestr/ui";
import { FileSearch, FolderOpen } from "lucide-react";
import { forwardRef, useState } from "react";

import { useT } from "../i18n";
import { canPickPaths, pickFile, pickFolder } from "../lib/tauri";

export interface PathInputProps extends Omit<InputProps, "value" | "onChange" | "rightSlot"> {
  value: string;
  onPathChange: (path: string) => void;
  /** What the dialog picks (default: a folder; `both` adds a second button for a file). */
  kind?: "folder" | "file" | "both";
  /** Native dialog title (default: "Choose a folder" / "Choose a file"). */
  dialogTitle?: string;
  /**
   * Maps the picked absolute path to the field value (e.g. relative to the
   * project folder). Return `null` to ignore the pick.
   */
  mapPicked?: (absolute: string) => string | null;
  /** Folder the dialog starts in (default: the current value). */
  defaultPath?: string;
}

export const PathInput = forwardRef<HTMLInputElement, PathInputProps>(function PathInput(
  { value, onPathChange, kind = "folder", dialogTitle, mapPicked, defaultPath, disabled, ...rest },
  ref,
) {
  const { t } = useT("workbench");
  const [picking, setPicking] = useState(false);
  const native = canPickPaths();

  const browse = async (what: "folder" | "file") => {
    setPicking(true);
    try {
      const opts = { title: dialogTitle ?? (what === "folder" ? t("pick.folder") : t("pick.file")), defaultPath: (defaultPath ?? value.trim()) || undefined };
      const picked = what === "folder" ? await pickFolder(opts) : await pickFile(opts);
      if (!picked) return;
      const next = mapPicked ? mapPicked(picked) : picked;
      if (next !== null) onPathChange(next);
    } catch (e) {
      console.warn("native dialog failed", e);
    } finally {
      setPicking(false);
    }
  };

  return (
    <Input
      ref={ref}
      {...rest}
      disabled={disabled}
      value={value}
      spellCheck={false}
      onChange={(e) => onPathChange(e.target.value)}
      rightSlot={
        native ? (
          <span className="path-input__actions">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              className="path-input__browse"
              leftIcon={kind === "file" ? <FileSearch size={13} /> : <FolderOpen size={13} />}
              loading={picking}
              disabled={disabled}
              title={kind === "file" ? t("browseFileAria") : t("browseFolderAria")}
              onClick={() => void browse(kind === "file" ? "file" : "folder")}
            >
              {t("browse")}
            </Button>
            {kind === "both" && (
              <IconButton
                size="sm"
                className="path-input__browse"
                label={t("browseFileAria")}
                icon={<FileSearch size={13} />}
                disabled={disabled || picking}
                onClick={() => void browse("file")}
              />
            )}
          </span>
        ) : undefined
      }
    />
  );
});
