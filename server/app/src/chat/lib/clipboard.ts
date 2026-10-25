/*
 * The async clipboard is not available to us.
 *
 * `navigator.clipboard.writeText` rejects with NotAllowedError inside the app's
 * webview — the platform does not consider a `tauri://` document a context it
 * will hand the clipboard to, so a copy that works in a browser fails here and
 * the user sees "the request is not allowed by the user agent". The older
 * `execCommand` path has no such rule and still runs, as long as it is reached
 * while the click that asked for the copy is live.
 *
 * So: try the modern call once, and once it has refused, stop asking.
 */
let asyncWriteWorks = true;

export async function copyText(value: string): Promise<void> {
  if (asyncWriteWorks && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {
      asyncWriteWorks = false;
    }
  }
  if (!writeBySelection(value))
    throw new Error("the platform would not take the clipboard");
}

/** Reading has no fallback — nothing lets a page take the clipboard unasked. */
export async function readClipboardText(): Promise<string> {
  return navigator.clipboard.readText();
}

/*
 * The text has to be in the document and selectable for the copy to have
 * anything to take, so it cannot be hidden with `display` or `visibility`.
 * Parked off to the left instead, and taken out again straight after.
 */
function writeBySelection(value: string): boolean {
  const holder = document.createElement("textarea");
  holder.value = value;
  holder.setAttribute("readonly", "");
  holder.style.cssText = "position:fixed;top:0;left:-9999px;opacity:0";
  document.body.appendChild(holder);

  const selection = document.getSelection();
  const previous =
    selection && selection.rangeCount > 0 ? selection.getRangeAt(0) : null;

  holder.select();
  let copied: boolean;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }

  holder.remove();
  if (selection && previous) {
    selection.removeAllRanges();
    selection.addRange(previous);
  }
  return copied;
}
