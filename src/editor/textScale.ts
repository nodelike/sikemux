export const DEFAULT_EDITOR_TEXT_SCALE = 1;
const MIN_EDITOR_TEXT_SCALE = 0.7;
const MAX_EDITOR_TEXT_SCALE = 2;

export function clampEditorTextScale(value: number): number {
    if (!Number.isFinite(value)) return DEFAULT_EDITOR_TEXT_SCALE;
    const rounded = Math.round(value * 100) / 100;
    return Math.min(MAX_EDITOR_TEXT_SCALE, Math.max(MIN_EDITOR_TEXT_SCALE, rounded));
}

/* Read by the CodeMirror theme's font size. Set on the root so an editor opened
   later starts at the size the reader already chose, and so changing it costs a
   style recalculation rather than a rebuilt extension. */
export function applyEditorTextScale(value: number): void {
    document.documentElement.style.setProperty("--editor-text-scale", String(clampEditorTextScale(value)));
}
