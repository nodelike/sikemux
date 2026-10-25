const userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent;
const platform = typeof navigator === "undefined" ? "" : navigator.platform;

export const IS_WINDOWS = /Windows/i.test(userAgent) || /^Win/i.test(platform);
export const IS_MACOS = /Mac/i.test(userAgent) || /^Mac/i.test(platform);

export function hasPrimaryModifier(event: Pick<KeyboardEvent, "metaKey" | "ctrlKey">): boolean {
    return IS_MACOS ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

export const PRIMARY_SHORTCUT = IS_MACOS ? "⌘" : "Ctrl+";
export const SHIFT_SHORTCUT = IS_MACOS ? "⇧" : "Shift+";
export const FILE_MANAGER_NAME = IS_MACOS ? "Finder" : IS_WINDOWS ? "File Explorer" : "file manager";
