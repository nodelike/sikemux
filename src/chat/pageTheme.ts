import { pagesApi } from "../api/pages";
import { swallow } from "../state/toast";
import { currentTheme, subscribeTheme } from "../themes/bus";
import type { Theme } from "../themes";

/** The variables a page styles against, named as the guide's `pages` topic lists them. */
export function pageThemeVariables(theme: Theme): Record<string, string> {
    const { chrome, editor, terminal } = theme;
    return {
        "--background": chrome.bg,
        "--foreground": chrome.ink,
        "--muted-foreground": chrome.inkDim,
        "--faint-foreground": chrome.inkMuted,
        "--card": chrome.bgRaised,
        "--border": chrome.line,
        "--accent": chrome.acc,
        "--accent-foreground": chrome.bgDim,
        "--accent-surface": chrome.accDim,
        "--destructive": chrome.danger,
        "--success": terminal.green,
        "--warning": terminal.yellow,
        "--info": terminal.blue,
        "--code-background": editor.bg,
        "--code-foreground": editor.fg,
        "--chart-1": chrome.acc,
        "--chart-2": terminal.blue,
        "--chart-3": terminal.green,
        "--chart-4": terminal.yellow,
        "--chart-5": terminal.magenta,
        "--chart-6": terminal.cyan,
        "--radius": "8px",
        "--font-sans": '-apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif',
        "--font-mono": '"SF Mono", Menlo, ui-monospace, monospace',
    };
}

/** The message that hands a framed page a new theme, in the MCP Apps shape. */
export function pageThemeMessage(theme: Theme) {
    return {
        jsonrpc: "2.0",
        method: "ui/notifications/host-context-changed",
        params: { theme: theme.dark ? "dark" : "light", styles: { variables: pageThemeVariables(theme) } },
    } as const;
}

let started = false;

/* Pages are themed as they are served, so the app keeps the current theme. */
export function startPageTheme(): void {
    if (started) return;
    started = true;
    const send = (theme: Theme) => void pagesApi.theme(theme.dark, pageThemeVariables(theme)).catch(swallow("page theme"));
    send(currentTheme());
    subscribeTheme(send);
}
