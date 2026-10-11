/* Agents showing their chat rather than their terminal. A page an agent shows
   goes in its chat, or on its desk when there is no chat to hold it. */
const chats = new Set<string>();

export function showingChat(agentId: string): () => void {
    chats.add(agentId);
    return () => chats.delete(agentId);
}

export function chatShown(agentId: string): boolean {
    return chats.has(agentId);
}
