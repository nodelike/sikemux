export const DEFAULT_CHAT_TEXT_SCALE = 1;
const MIN_CHAT_TEXT_SCALE = 0.7;
const MAX_CHAT_TEXT_SCALE = 2;

export function clampChatTextScale(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_CHAT_TEXT_SCALE;
    const rounded = Math.round(value * 100) / 100;
    return Math.min(MAX_CHAT_TEXT_SCALE, Math.max(MIN_CHAT_TEXT_SCALE, rounded));
}

/* Read by `--chat-text-em` in chat.css, which every text size in the transcript
   is multiplied by. Set on the root so panes mounted later start at the size
   the user already chose. */
export function applyChatTextScale(value: number): void {
    document.documentElement.style.setProperty("--chat-text-scale", String(clampChatTextScale(value)));
}
