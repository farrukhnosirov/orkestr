/**
 * Left-rail entries of the Window Library: the panels pinned from the
 * library ("Pin to sidebar") and the button that opens the library itself.
 */
import { IconButton } from "@orkestr/ui";
import { LibraryBig } from "lucide-react";
import { useMemo } from "react";

import { useLanguage, useT } from "../../i18n";
import { openHere } from "./actions";
import { libraryEntries } from "./catalog";
import { openWindowLibrary, useWindowLibraryStore } from "./store";
import { WINDOW_LIBRARY_SHORTCUT } from "./commands";

/** Pinned panels (one icon button each). */
export function RailPanelPins() {
  const { t } = useT("windowLibrary");
  const lang = useLanguage();
  const pins = useWindowLibraryStore((s) => s.pins);
  const entries = useMemo(() => libraryEntries(), [lang]); // eslint-disable-line react-hooks/exhaustive-deps
  const pinned = pins.map((id) => entries.find((e) => e.id === id)).filter((e) => e !== undefined);
  if (pinned.length === 0) return null;
  return (
    <div className="rail__pins wl-rail-pins" role="group" aria-label={t("rail.label")}>
      {pinned.map((entry) => {
        const Icon = entry.icon;
        return (
          <IconButton
            key={entry.id}
            className="wl-rail-pin"
            data-wl-pin={entry.id}
            label={t("rail.open", { title: entry.title })}
            icon={<Icon size={16} />}
            tooltip="right"
            onClick={() => openHere(entry)}
          />
        );
      })}
    </div>
  );
}

/** Rail button opening the Window Library. */
export function RailLibraryButton() {
  const { t } = useT("windowLibrary");
  return (
    <IconButton
      label={`${t("rail.library")} (${WINDOW_LIBRARY_SHORTCUT})`}
      icon={<LibraryBig size={16} />}
      onClick={openWindowLibrary}
      tooltip="right"
    />
  );
}
