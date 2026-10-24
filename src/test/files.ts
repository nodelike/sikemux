/** The read of a file on this host that the editor asks the core for, or null when `command` is something else. */
export function fileRead(command: unknown, args: unknown): { path: string; at: number } | null {
    if (command !== "core_call") return null;
    const { method, params } = args as { method: string; params: { path: string; at: number } };
    return method === "files.read" ? params : null;
}

/** A whole file as the one piece a read answers with. */
export function wholeFile(content: string, version: string) {
    return { text: content, next: content.length, size: content.length, stamp: "stamp", version };
}
