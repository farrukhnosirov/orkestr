/** expo-audio recorder as a core `Recorder` (hook: expo-audio recorders are hook-owned). */
import { RecordingPresets, requestRecordingPermissionsAsync, setAudioModeAsync, useAudioRecorder } from "expo-audio";
import { useMemo } from "react";

import type { Recorder, Recording } from "../core";

async function fileToBase64(uri: string): Promise<string> {
  const blob = await (await fetch(uri)).blob();
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onerror = () => reject(new Error("could not read the recording"));
    r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
    r.readAsDataURL(blob);
  });
}

export function useExpoRecorder(): Recorder {
  const rec = useAudioRecorder(RecordingPresets.HIGH_QUALITY);
  return useMemo<Recorder>(
    () => ({
      async start() {
        const p = await requestRecordingPermissionsAsync();
        if (!p.granted) throw Object.assign(new Error("microphone permission denied"), { code: "permission" });
        await setAudioModeAsync({ allowsRecording: true, playsInSilentMode: true });
        await rec.prepareToRecordAsync();
        rec.record();
      },
      async stop(): Promise<Recording> {
        await rec.stop();
        await setAudioModeAsync({ allowsRecording: false }).catch(() => undefined);
        if (!rec.uri) throw new Error("no recording");
        // HIGH_QUALITY = AAC in an MP4 container (.m4a) on both platforms.
        return { base64: await fileToBase64(rec.uri), mimeType: "audio/mp4" };
      },
    }),
    [rec],
  );
}
