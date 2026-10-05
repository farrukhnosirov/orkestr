/** Registers the layout feature's commands in the shared registry (idempotent). */
import { commands } from "../commands/registry";
import { layoutCommands } from "./commands";
import { bindMissionControlToLayout } from "./missionControlBinding";

let registered = false;

export function registerLayoutFeature(): void {
  if (registered) return;
  registered = true;
  commands.push(...layoutCommands);
  // Mission Control's widget arrangement is stored with the active layout (per device).
  bindMissionControlToLayout();
}
