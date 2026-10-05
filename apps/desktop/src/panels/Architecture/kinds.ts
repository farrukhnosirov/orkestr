/** Kind icons and CSS classes shared by the map, list and detail views. */
import type { ArchNodeKind } from "@orkestr/protocol";
import { Boxes, Database, type LucideIcon, Package, Server, Webhook } from "lucide-react";

export const KIND_ICON: Record<ArchNodeKind, LucideIcon> = {
  Service: Server,
  Module: Package,
  Api: Webhook,
  Database,
  External: Boxes,
};

/** CSS class carrying `--arch-kind-color` for a kind. */
export function kindClass(kind: ArchNodeKind): string {
  return `arch-kind--${kind.toLowerCase()}`;
}

/** CSS variable holding the colour token of a kind (used by the SVG export). */
export const KIND_TOKEN: Record<ArchNodeKind, string> = {
  Service: "--ok-color-accent",
  Module: "--ok-color-violet",
  Api: "--ok-color-info",
  Database: "--ok-color-warning",
  External: "--ok-color-text-muted",
};
