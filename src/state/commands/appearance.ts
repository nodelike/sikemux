import { invokeCommand as invoke } from "../../api/invoke";
import { cloneTheme, DEFAULT_THEME_ID, type Theme } from "../../themes";
import { applyTheme, applyWindowOpacity, previewTheme, registerCustomThemes } from "../../themes/bus";
import { applyTerminalFontSize, clampTerminalFontSize, DEFAULT_TERMINAL_FONT_SIZE } from "../../terminal/fontSize";
import { applyChatTextScale, clampChatTextScale, DEFAULT_CHAT_TEXT_SCALE } from "../../chat/textScale";
import { applyEditorTextScale, clampEditorTextScale, DEFAULT_EDITOR_TEXT_SCALE } from "../../editor/textScale";
import { getState, setState } from "../store";
import { swallow } from "../toast";

export function setThemeId(id: string): void {
    applyTheme(id);
    setState({ themeId: id });
}

/** Live-apply a draft theme to the whole UI without persisting it — drives the theme editor preview. */
export function previewThemeDraft(theme: Theme): void {
    previewTheme(theme);
}

/** Discard any active preview and re-apply the persisted theme selection. */
export function cancelThemePreview(): void {
    applyTheme(getState().themeId);
}

/** Insert or overwrite a custom theme (matched by id), register it, and make it the active theme. */
export function saveCustomTheme(theme: Theme): void {
    setState((s) => {
        const idx = s.customThemes.findIndex((t) => t.id === theme.id);
        const customThemes = idx >= 0 ? s.customThemes.map((t, i) => (i === idx ? theme : t)) : [...s.customThemes, theme];
        return { customThemes };
    });
    registerCustomThemes(getState().customThemes);
    setThemeId(theme.id);
}

export function deleteCustomTheme(id: string): void {
    setState((s) => ({ customThemes: s.customThemes.filter((t) => t.id !== id) }));
    registerCustomThemes(getState().customThemes);
    if (getState().themeId === id) setThemeId(DEFAULT_THEME_ID);
}

export function duplicateCustomTheme(id: string): void {
    const src = getState().customThemes.find((t) => t.id === id);
    if (!src) return;
    saveCustomTheme(cloneTheme(src, { id: `custom-${Date.now().toString(36)}`, name: `${src.name} copy` }));
}

export function setTerminalFontSize(v: number): void {
    const value = clampTerminalFontSize(v);
    applyTerminalFontSize(value);
    setState({ terminalFontSize: value });
}

export function adjustTerminalFontSize(step: number): void {
    setTerminalFontSize(getState().terminalFontSize + step);
}

export function resetTerminalFontSize(): void {
    setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE);
}

export function setChatTextScale(v: number): void {
    const value = clampChatTextScale(v);
    applyChatTextScale(value);
    setState({ chatTextScale: value });
}

export function adjustChatTextScale(step: number): void {
    setChatTextScale(getState().chatTextScale + step);
}

export function resetChatTextScale(): void {
    setChatTextScale(DEFAULT_CHAT_TEXT_SCALE);
}

export function setEditorTextScale(v: number): void {
    const value = clampEditorTextScale(v);
    applyEditorTextScale(value);
    setState({ editorTextScale: value });
}

export function adjustEditorTextScale(step: number): void {
    setEditorTextScale(getState().editorTextScale + step);
}

export function resetEditorTextScale(): void {
    setEditorTextScale(DEFAULT_EDITOR_TEXT_SCALE);
}

export function setWindowOpacity(v: number): void {
    const value = Number.isFinite(v) ? v : 1;
    applyWindowOpacity(value);
    setState({ windowOpacity: value });
}

export function setWindowBlur(v: number): void {
    const value = Number.isFinite(v) ? Math.round(v) : 0;
    void invoke("set_window_blur", { radius: value }).catch(swallow("set_window_blur"));
    setState({ windowBlur: value });
}
