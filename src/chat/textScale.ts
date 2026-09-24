export const DEFAULT_CHAT_TEXT_SCALE = 1;
const MIN_CHAT_TEXT_SCALE = 0.7;
const MAX_CHAT_TEXT_SCALE = 2;

export function clampChatTextScale(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_CHAT_TEXT_SCALE;
    const rounded = Math.round(value * 100) / 100;
    return Math.min(MAX_CHAT_TEXT_SCALE, Math.max(MIN_CHAT_TEXT_SCALE, rounded));
}

// Set on the root so a chat opened later starts at the chosen size.
export function applyChatTextScale(value: number): void {
    document.documentElement.style.setProperty("--chat-text-scale", String(clampChatTextScale(value)));
}
