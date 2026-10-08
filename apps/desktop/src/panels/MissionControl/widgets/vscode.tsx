/** Mission Control quick actions: pinned VS Code extension commands. */
import { PinnedQuickActions } from "../../../features/vscode/PinnedCommands";
import type { WidgetProps } from "../context";

export function VsCodeActionsWidget(_props: WidgetProps) {
  return <PinnedQuickActions />;
}
