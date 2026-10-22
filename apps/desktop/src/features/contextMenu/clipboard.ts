/**
 * Clipboard access for the context menu (and anyone else in the UI).
 *
 * The async Clipboard API is used first. In Orkestr's own windows the shell
 * grants `clipboard-read` to the bundled UI's origin (no Edge permission
 * prompt, see `orkestr_platform::webview_shell`); writes need only the user
 * gesture of the menu click. When the API is missing or refused, writes fall
 * back to a hidden textarea + `execCommand("copy")`; reads report `null`
 * (the caller then does nothing — the keyboard shortcut still works).
 */

/** Copies text. Resolves `true` when the clipboard was written. */
export async function writeClipboardText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall back below */
  }
  return copyViaTextarea(text);
}

function copyViaTextarea(text: string): boolean {
  if (typeof document === "undefined" || typeof document.execCommand !== "function") return false;
  const active = document.activeElement as HTMLElement | null;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  ta.style.pointerEvents = "none";
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  active?.focus?.();
  return ok;
}

/** Reads text, `null` when the clipboard is unavailable or refused. */
export async function readClipboardText(): Promise<string | null> {
  try {
    if (navigator.clipboard?.readText) return await navigator.clipboard.readText();
  } catch {
    /* refused */
  }
  return null;
}

/**
 * Copies an image (as PNG) — the image must be same-origin or a data/blob
 * URL (a tainted canvas cannot be exported). Resolves `true` on success.
 */
export async function writeClipboardImage(img: HTMLImageElement): Promise<boolean> {
  try {
    if (typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) return false;
    const blob = await imageToPng(img);
    if (!blob) return false;
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
    return true;
  } catch {
    return false;
  }
}

function imageToPng(img: HTMLImageElement): Promise<Blob | null> {
  return new Promise((resolve) => {
    const w = img.naturalWidth || img.width;
    const h = img.naturalHeight || img.height;
    if (!w || !h) return resolve(null);
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return resolve(null);
    try {
      ctx.drawImage(img, 0, 0, w, h);
      canvas.toBlob((b) => resolve(b), "image/png");
    } catch {
      resolve(null);
    }
  });
}

/** A file name for "Save image as…" from the image URL. */
export function imageFileName(src: string): string {
  if (src.startsWith("data:")) {
    const ext = /^data:image\/([a-z0-9.+-]+)/i.exec(src)?.[1]?.replace("svg+xml", "svg") ?? "png";
    return `image.${ext}`;
  }
  try {
    const name = new URL(src, "http://x/").pathname.split("/").filter(Boolean).pop();
    return name && /\.[a-z0-9]{2,5}$/i.test(name) ? decodeURIComponent(name) : "image.png";
  } catch {
    return "image.png";
  }
}

/** "Save image as…": hands the image to the web view's download (save dialog). */
export function saveImage(src: string): void {
  const a = document.createElement("a");
  a.href = src;
  a.download = imageFileName(src);
  a.rel = "noopener";
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  a.remove();
}
