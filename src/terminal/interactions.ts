export interface TerminalSearchOptions {
    caseSensitive: boolean;
    regex: boolean;
    wholeWord: boolean;
}

export function isTerminalFindShortcut(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, macos: boolean): boolean {
    return event.code === "KeyF" && !event.altKey && !event.shiftKey && (macos ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey);
}
