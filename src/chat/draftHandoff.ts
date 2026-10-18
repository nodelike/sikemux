/** An agent's chat composer, as the agent's view sees it when switching between GUI and TUI. */
interface Composer {
    take: () => string;
    put: (text: string) => void;
}

const composers = new Map<string, Composer>();

export function registerComposer(agentId: string, composer: Composer): () => void {
    composers.set(agentId, composer);
    return () => {
        if (composers.get(agentId) === composer) composers.delete(agentId);
    };
}

/** What is typed in the agent's composer, emptied as it is taken. */
export function takeDraft(agentId: string): string {
    return composers.get(agentId)?.take() ?? "";
}

/** Adds `text` to the agent's composer, after anything already in it. */
export function putDraft(agentId: string, text: string): boolean {
    const composer = composers.get(agentId);
    if (!composer || !text.trim()) return false;
    composer.put(text);
    return true;
}
