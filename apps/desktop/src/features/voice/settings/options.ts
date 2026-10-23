/** Option lists for voice settings. */

/** OpenAI TTS voices. */
export const CLOUD_VOICES: readonly string[] = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"];

/** Recognition / speech languages (`auto` = UI language). */
export const LANGUAGE_OPTIONS = ["auto", "ru-RU", "en-US", "en-GB"] as const;

/** Push-to-talk hold hotkeys offered in Settings. */
export const PTT_HOTKEYS: readonly string[] = ["Ctrl+Space", "Ctrl+Shift+Space", "Ctrl+Alt+Space", "F9", "Ctrl+Shift+V"];

/** Piper voice packages (ADR-0046). */
export const PIPER_VOICES: readonly string[] = ["piper-ru-irina", "piper-ru-denis", "piper-ru-dmitri", "piper-ru-ruslan"];
