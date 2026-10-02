/* Chats this page shows. The background core keeps chats running across a
   reload or a quit, and one no chat pane takes up again is stopped. */
const claimed = new Set<string>();

export function claimChat(agentId: string): void {
    claimed.add(agentId);
}

export function chatClaimed(agentId: string): boolean {
    return claimed.has(agentId);
}
