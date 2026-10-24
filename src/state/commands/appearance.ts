import { invokeCommand as invoke } from "../../api/invoke";
import { coreCall } from "../../api/core";
import { DEFAULT_THEME_ID, type Theme } from "../../themes";
import { applyWindowOpacity, endPreview, previewTheme, previewThemeId } from "../../themes/bus";
import { applyTerminalFontSize, clampTerminalFontSize, DEFAULT_TERMINAL_FONT_SIZE } from "../../terminal/fontSize";
import { applyChatTextScale, clampChatTextScale, DEFAULT_CHAT_TEXT_SCALE } from "../../chat/textScale";
import { applyEditorTextScale, clampEditorTextScale, DEFAULT_EDITOR_TEXT_SCALE } from "../../editor/textScale";
import { setPreference } from "../preferences";
import { getState } from "../store";
import { swallow } from "../toast";

export const themeIdNow = (): string => getState().preferences?.themeId ?? DEFAULT_THEME_ID;
const customThemesNow = (): Theme[] => getState().preferences?.customThemes ?? [];

/** Picks a theme; the window wears it once the core has resolved it. */
export function setThemeId(id: string): void {
    setPreference("themeId", id);
}

/** Live-apply a draft theme to the whole UI without persisting it — drives the theme editor preview. */
export function previewThemeDraft(theme: Theme): void {
    previewTheme(theme);
}

/** Live-apply one of the themes to pick from, without picking it. */
export function previewThemeById(id: string): void {
    previewThemeId(id);
}

/** Discard any active preview and wear the picked theme again. */
export function cancelThemePreview(): void {
    endPreview();
}

/** A theme the core works out from a wallpaper's pixels, as base64 RGBA. */
export function themeFromPixels(name: string, pixels: string): Promise<Theme> {
    return coreCall<Theme>("themeFromPixels", { name, pixels });
}

/** Insert or overwrite a custom theme (matched by id) and make it the active theme. */
export function saveCustomTheme(theme: Theme): void {
    const kept = customThemesNow();
    const customThemes = kept.some((t) => t.id === theme.id) ? kept.map((t) => (t.id === theme.id ? theme : t)) : [...kept, theme];
    setPreference("customThemes", customThemes);
    setThemeId(theme.id);
}

export function deleteCustomTheme(id: string): void {
    const customThemes = customThemesNow().filter((t) => t.id !== id);
    setPreference("customThemes", customThemes);
    if (themeIdNow() === id) setThemeId(DEFAULT_THEME_ID);
}

export function setTerminalFontSize(v: number): void {
    const value = clampTerminalFontSize(v);
    applyTerminalFontSize(value);
    setPreference("terminalFontSize", value);
}

export function adjustTerminalFontSize(step: number): void {
    setTerminalFontSize(terminalFontSize() + step);
}

export function resetTerminalFontSize(): void {
    setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE);
}

export function setChatTextScale(v: number): void {
    const value = clampChatTextScale(v);
    applyChatTextScale(value);
    setPreference("chatTextScale", value);
}

export function adjustChatTextScale(step: number): void {
    setChatTextScale(chatTextScale() + step);
}

export function resetChatTextScale(): void {
    setChatTextScale(DEFAULT_CHAT_TEXT_SCALE);
}

export function setEditorTextScale(v: number): void {
    const value = clampEditorTextScale(v);
    applyEditorTextScale(value);
    setPreference("editorTextScale", value);
}

export function adjustEditorTextScale(step: number): void {
    setEditorTextScale(editorTextScale() + step);
}

export const terminalFontSize = (): number => getState().preferences?.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE;
export const chatTextScale = (): number => getState().preferences?.chatTextScale ?? DEFAULT_CHAT_TEXT_SCALE;
export const editorTextScale = (): number => getState().preferences?.editorTextScale ?? DEFAULT_EDITOR_TEXT_SCALE;

export function resetEditorTextScale(): void {
    setEditorTextScale(DEFAULT_EDITOR_TEXT_SCALE);
}

export function setWindowOpacity(v: number): void {
    const value = Number.isFinite(v) ? v : 1;
    applyWindowOpacity(value);
    setPreference("windowOpacity", value);
}

let blurred: number | null = null;

export function applyWindowBlur(radius: number): void {
    if (radius === blurred) return;
    blurred = radius;
    void invoke("set_window_blur", { radius }).catch(swallow("set_window_blur"));
}

export function setWindowBlur(v: number): void {
    const value = Number.isFinite(v) ? Math.round(v) : 0;
    applyWindowBlur(value);
    setPreference("windowBlur", value);
}
