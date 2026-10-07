/**
 * File / folder icon: the active VS Code file icon theme when one is
 * selected (Settings → Editor), else Orkestr's built-in icons.
 */
import "./extensions.css";

import { useEffect } from "react";

import { fileIcon } from "../files/icons";
import { languageOfName, requestIcon, resolveIconId, useIconThemeStore } from "./iconTheme";
import { useExtensionsStore } from "./store";

export interface FileTypeIconProps {
  name: string;
  kind: "file" | "dir";
  open?: boolean;
  size?: number;
}

export function FileTypeIcon({ name, kind, open = false, size = 14 }: FileTypeIconProps) {
  const associations = useIconThemeStore((s) => s.associations);
  const installed = useExtensionsStore((s) => s.installed);
  const id = associations ? resolveIconId(associations, name, kind, open, kind === "file" ? languageOfName(name, installed) : null) : null;
  const icon = useIconThemeStore((s) => (id ? s.icons[id] : undefined));

  useEffect(() => {
    if (id && icon === undefined) requestIcon(id);
  }, [id, icon]);

  if (icon?.kind === "image") {
    return <img className="ok-file-icon" src={icon.src} width={size} height={size} alt="" aria-hidden="true" draggable={false} />;
  }
  if (icon?.kind === "glyph") {
    return (
      <span
        className="ok-file-icon ok-file-icon--glyph"
        aria-hidden="true"
        style={{
          fontFamily: icon.family,
          color: icon.color ?? undefined,
          fontSize: icon.size?.trim().endsWith("%") ? `calc(${size}px * ${parseFloat(icon.size) / 100 || 1})` : (icon.size ?? size),
          width: size,
          height: size,
          lineHeight: `${size}px`,
        }}
      >
        {icon.char}
      </span>
    );
  }
  const { icon: Fallback, color } = fileIcon(name, kind, open);
  return <Fallback size={size} style={{ color }} aria-hidden="true" />;
}
