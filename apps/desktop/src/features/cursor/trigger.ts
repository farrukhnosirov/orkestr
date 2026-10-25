/**
 * Voice / text triggers of cursor capture mode (ADR-0050):
 * «Джарвис, возьми управление и открой настройки блютуз»,
 * «захвати курсор», «take control and close all windows except VS Code».
 * Returns `null` when the utterance is not a trigger, otherwise the task
 * after the trigger ("" when only the trigger was said).
 */
const WAKE = /^(?:(?:эй|ok|окей|hey)[,\s]+)?(?:джарвис|jarvis|оркестр|orkestr)[,!.\s]*/i;
const TRIGGER =
  /^(?:возьми|бери|взять|прими)\s+(?:на\s+себя\s+)?(?:управление|контроль)(?:\s+(?:курсором|мышью|компьютером))?|^(?:захвати|перехвати|возьми)\s+(?:курсор|мышь|мышку)|^управляй\s+(?:курсором|мышью|компьютером)|^режим\s+(?:управления\s+курсором|захвата\s+курсора)|^take\s+(?:the\s+)?(?:control|over)(?:\s+(?:of\s+)?(?:the\s+)?(?:cursor|mouse|computer))?|^(?:grab|capture)\s+(?:the\s+)?(?:cursor|mouse)|^cursor\s+mode/i;

export function parseCursorTrigger(text: string): string | null {
  const s = text.trim().replace(WAKE, "");
  const m = TRIGGER.exec(s);
  if (!m) return null;
  return s
    .slice(m[0].length)
    .replace(/^[\s,.:;!—–-]+/, "")
    .replace(/^(?:и|and|then|затем|чтобы|to)\s+/i, "")
    .trim();
}
