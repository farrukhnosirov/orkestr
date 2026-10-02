/**
 * Localized file-name validation for the Files panel. The rules live in
 * `features/files/paths.ts` (`validateName`, shared and English); this maps
 * its messages to translated texts and passes unknown ones through unchanged.
 */
import { t } from "../../i18n";
import { validateName } from "../../features/files/paths";

type ValidationKey =
  | "files:validation.empty"
  | "files:validation.reserved"
  | "files:validation.badChars"
  | "files:validation.dotDot"
  | "files:validation.trailing"
  | "files:validation.windows";

const KEYS: Readonly<Record<string, ValidationKey>> = {
  "Enter a name.": "files:validation.empty",
  "That name is reserved.": "files:validation.reserved",
  'Names cannot contain < > : " | ? * or control characters.': "files:validation.badChars",
  "Names cannot contain `..`.": "files:validation.dotDot",
  "Names cannot end with a dot or a space.": "files:validation.trailing",
  "That name is reserved on Windows.": "files:validation.windows",
};

/** Why `name` is not a valid file/folder name (in the UI language), or null. */
export function nameError(name: string): string | null {
  const message = validateName(name);
  if (message === null) return null;
  const key = KEYS[message];
  return key ? t(key) : message;
}
