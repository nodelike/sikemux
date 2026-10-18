/*
 * Stands in for @tauri-apps/api/core when the Mac's chat view runs in a browser. A shared chat
 * names files on its sharer's computer, so nothing here reaches a disk; a link opens in a tab.
 */
export function invoke(
  command: string,
  args?: Record<string, unknown>,
): Promise<never> {
  if (command === "open_url" && typeof args?.url === "string")
    window.open(args.url, "_blank", "noopener,noreferrer");
  return Promise.reject(new Error(`${command} is not available in a browser`));
}

export function convertFileSrc(): string {
  return "";
}
