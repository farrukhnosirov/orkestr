/** Settings → "Remote & Sync": remote access (devices) + encrypted project sync. */
import type { SettingsSectionProps } from "../settings/sections";
import { RemotePanel } from "./RemotePanel";
import { SyncPanel } from "./SyncPanel";
import "./remoteSync.css";

export function RemoteSyncSettings({ projectId }: SettingsSectionProps) {
  return (
    <div className="rs-settings">
      <SyncPanel projectId={projectId} />
      <RemotePanel />
    </div>
  );
}
