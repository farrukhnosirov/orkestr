/** Maps an assistant run onto the voice pipeline's command result (HUD + spoken reply). */
import type { CommandResult } from "../voice/pipeline/executor";
import type { AssistantRun } from "./api";

export function assistantResult(r: AssistantRun): CommandResult {
  const outcome: CommandResult["outcome"] =
    r.outcome === "ok"
      ? "ok"
      : r.outcome === "needsConfirmation" || r.outcome === "proposed"
        ? "needsConfirmation"
        : r.outcome === "cancelled"
          ? "cancelled"
          : r.outcome === "notUnderstood"
            ? "notUnderstood"
            : "failed";
  return { outcome, reply: r.reply, tone: r.tone, projectId: null };
}
