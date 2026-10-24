import { DEFAULT_TEXT_SCALE, MAX_TEXT_SCALE, MIN_TEXT_SCALE } from "../api/generated/wire";

export const DEFAULT_CHAT_TEXT_SCALE = DEFAULT_TEXT_SCALE;

export function clampChatTextScale(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_CHAT_TEXT_SCALE;
    const rounded = Math.round(value * 100) / 100;
    return Math.min(MAX_TEXT_SCALE, Math.max(MIN_TEXT_SCALE, rounded));
}

// Set on the root so a chat opened later starts at the chosen size.
export function applyChatTextScale(value: number): void {
    document.documentElement.style.setProperty("--chat-text-scale", String(clampChatTextScale(value)));
}
