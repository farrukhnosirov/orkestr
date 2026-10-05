/** Registers the layout feature's commands in the shared registry (idempotent). */
import { commands } from "../commands/registry";
import { layoutCommands } from "./commands";

let registered = false;

export function registerLayoutFeature(): void {
  if (registered) return;
  registered = true;
  commands.push(...layoutCommands);
}
