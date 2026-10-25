import { DEFAULT_TEXT_SCALE, MAX_TEXT_SCALE, MIN_TEXT_SCALE } from "@shared/wire.generated";

export const DEFAULT_EDITOR_TEXT_SCALE = DEFAULT_TEXT_SCALE;

export function clampEditorTextScale(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_EDITOR_TEXT_SCALE;
    const rounded = Math.round(value * 100) / 100;
    return Math.min(MAX_TEXT_SCALE, Math.max(MIN_TEXT_SCALE, rounded));
}

// Set on the root so an editor opened later starts at the chosen size, without rebuilding its theme.
export function applyEditorTextScale(value: number): void {
    document.documentElement.style.setProperty("--editor-text-scale", String(clampEditorTextScale(value)));
}
