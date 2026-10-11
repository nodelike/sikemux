import { useEffect } from "react";
import { browserApi } from "../api/browser";
import { actionForEvent, pluginOpenedBy, pluginShortcutFor, type KeybindingActionId } from "./keybindings";
import * as cmd from "../state/commands";
import { activeAgentId } from "../state/selectors";
import { getState, type StoreState } from "../state/store";
import type { KeyModifier } from "../state/types";
import { runMeasuredAction } from "../lib/instrumentation";
import { applicationActionContext, executeApplicationAction, matchApplicationActionKeybinding } from "../actions/bridge";
import { reportError } from "../state/toast";
import { pluginOverlayOpen } from "../plugins/overlays";
import { frontendPlugin, pluginSurface } from "../plugins/registry";

const TEXT_SCALE_STEP = 0.1;

type TextSurface = "chat" | "editor" | "terminal";

/**
 * Which text a size key resizes. Clicking a chat transcript or a button that
 * then disables drops focus to the page, so the active pane stands in for it.
 */
function textSurfaceFor(e: KeyboardEvent, st: StoreState): TextSurface {
    if (keyTargetIn(e, ".agent-chat-pane")) return "chat";
    if (keyTargetIn(e, ".cm-editor")) return "editor";
    if (keyTargetIn(e, ".xterm")) return "terminal";
    const paneId = st.windows[st.sessions[st.activeSessionId]?.activeWindowId ?? ""]?.activePaneId;
    const pane = paneId ? document.querySelector(`[data-pane-id="${CSS.escape(paneId)}"]`) : null;
    if (pane?.querySelector(".agent-chat-pane")) return "chat";
    if (pane?.querySelector(".cm-editor")) return "editor";
    return "terminal";
}

// The command deck sends a synthetic event with no target, so the focused element stands in.
function keyTargetIn(e: KeyboardEvent, selector: string): boolean {
    const target = e.target instanceof Element ? e.target : document.activeElement;
    return !!target?.closest?.(selector);
}

/** A key pressed on a page, or a command deck run, which has no key behind it and means the page in front. */
function reachesBrowser(e: KeyboardEvent): boolean {
    return keyTargetIn(e, "[data-browser-pane]") || !e.isTrusted;
}

/** The agent whose desk the key was pressed in, if it was pressed in one. */
function deskKeyTarget(e: KeyboardEvent): string | null {
    const target = e.target instanceof Element ? e.target : document.activeElement;
    return target?.closest<HTMLElement>("[data-desk]")?.dataset.agentId ?? null;
}

function hasOpenModal(st: StoreState): boolean {
    return (
        st.pickerOpen ||
        st.agentPaletteOpen ||
        st.filePaletteOpen ||
        st.commandPaletteOpen ||
        st.newTabPaletteOpen ||
        st.commandPopup !== null ||
        st.onboardingOpen ||
        st.diagnosticsOpen ||
        st.whatsNewOpen ||
        pluginOverlayOpen()
    );
}

const MODAL_ACTIONS = new Set<KeybindingActionId>(["palette.commands", "palette.files", "search.global", "settings.toggle"]);
const KEEPS_SETTINGS = new Set<KeybindingActionId>([...MODAL_ACTIONS, "text.sizeIncrease", "text.sizeDecrease", "text.sizeReset"]);
// Closing is what these mean while settings covers the workspace, not closing something hidden behind it.
const CLOSES_SETTINGS = new Set<KeybindingActionId>(["pane.close", "session.close"]);

function releaseModifierForEvent(event: KeyboardEvent): KeyModifier | null {
    if (event.altKey) return "Alt";
    if (event.metaKey) return "Meta";
    if (event.ctrlKey) return "Control";
    if (event.shiftKey) return "Shift";
    return null;
}

function modifierHeld(event: KeyboardEvent, modifier: KeyModifier): boolean {
    if (modifier === "Alt") return event.altKey;
    if (modifier === "Meta") return event.metaKey;
    if (modifier === "Control") return event.ctrlKey;
    return event.shiftKey;
}

export function runKeybindingAction(action: KeybindingActionId, event: KeyboardEvent, st: StoreState): boolean {
    const active = st.sessions[st.activeSessionId];
    const opened = pluginOpenedBy(action);
    if (opened) {
        const plugin = frontendPlugin(opened);
        plugin?.open();
        return !!plugin;
    }
    const shortcut = pluginShortcutFor(action);
    if (shortcut) return shortcut.run();

    switch (action) {
        case "palette.commands":
            cmd.toggleCommandPalette();
            return true;
        case "palette.files": {
            const quickOpen = active ? pluginSurface(active.kind)?.quickOpen : undefined;
            if (quickOpen) {
                quickOpen();
            } else if (st.filePaletteOpen) {
                cmd.closeFilePalette();
            } else {
                cmd.openFilePalette();
            }
            return true;
        }
        case "search.global": {
            const selection = window.getSelection()?.toString() ?? "";
            cmd.focusGlobalSearch(selection.trim() ? selection : undefined);
            return true;
        }
        case "settings.toggle":
            cmd.toggleSettings();
            return true;
        case "view.focusMode":
            cmd.toggleZen();
            return true;
        case "pane.splitRow":
            // ⌘D adds the next match to the selection in the editor, which it keeps.
            if (event.metaKey && !event.shiftKey && !event.altKey && event.code === "KeyD" && keyTargetIn(event, ".cm-editor")) {
                return false;
            }
            cmd.splitActivePane("row");
            return true;
        case "pane.splitColumn":
            cmd.splitActivePane("column");
            return true;
        case "pane.splitStack":
            cmd.splitActivePane("stack");
            return true;
        case "pane.focusLeft":
            cmd.moveFocus("left");
            return true;
        case "pane.focusDown":
            cmd.moveFocus("down");
            return true;
        case "pane.focusUp":
            cmd.moveFocus("up");
            return true;
        case "pane.focusRight":
            cmd.moveFocus("right");
            return true;
        case "pane.resizeLeft":
            cmd.resizeActivePane("left");
            return true;
        case "pane.resizeDown":
            cmd.resizeActivePane("down");
            return true;
        case "pane.resizeUp":
            cmd.resizeActivePane("up");
            return true;
        case "pane.resizeRight":
            cmd.resizeActivePane("right");
            return true;
        case "pane.zoom":
            cmd.toggleZoom();
            return true;
        case "pane.close": {
            const deskAgentId = deskKeyTarget(event);
            if (deskAgentId) cmd.closeShownDeskTab(deskAgentId);
            else cmd.closeActiveFocusTarget();
            return true;
        }
        case "text.sizeIncrease": {
            const surface = textSurfaceFor(event, st);
            if (surface === "chat") cmd.adjustChatTextScale(TEXT_SCALE_STEP);
            else if (surface === "editor") cmd.adjustEditorTextScale(TEXT_SCALE_STEP);
            else cmd.adjustTerminalFontSize(1);
            return true;
        }
        case "text.sizeDecrease": {
            const surface = textSurfaceFor(event, st);
            if (surface === "chat") cmd.adjustChatTextScale(-TEXT_SCALE_STEP);
            else if (surface === "editor") cmd.adjustEditorTextScale(-TEXT_SCALE_STEP);
            else cmd.adjustTerminalFontSize(-1);
            return true;
        }
        case "text.sizeReset": {
            const surface = textSurfaceFor(event, st);
            if (surface === "chat") cmd.resetChatTextScale();
            else if (surface === "editor") cmd.resetEditorTextScale();
            else cmd.resetTerminalFontSize();
            return true;
        }
        case "agent.new":
            void cmd.startAgent();
            return true;
        case "agent.choose":
            cmd.chooseAgent();
            return true;
        case "desk.toggle":
            cmd.toggleNearestDesk();
            return true;
        case "terminal.new":
            cmd.newTerminal();
            return true;
        case "window.next":
            cmd.cycleTab(1);
            return true;
        case "window.previous":
            cmd.cycleTab(-1);
            return true;
        case "tab.next":
        case "tab.previous": {
            const delta = action === "tab.next" ? 1 : -1;
            const deskAgentId = deskKeyTarget(event);
            if (deskAgentId) cmd.cycleDeskTab(deskAgentId, delta);
            else cmd.cycleTabs(delta);
            return true;
        }
        case "tab.goto1":
        case "tab.goto2":
        case "tab.goto3":
        case "tab.goto4":
        case "tab.goto5":
        case "tab.goto6":
        case "tab.goto7":
        case "tab.goto8":
        case "tab.goto9":
            cmd.selectTabAt(Number(action.slice("tab.goto".length)));
            return true;
        case "project.open":
            cmd.openPicker("projects");
            return true;
        case "session.open":
            cmd.openPicker("all");
            return true;
        case "ssh.open":
            cmd.openPicker("ssh");
            return true;
        case "session.command":
            cmd.focusCommandSession();
            return true;
        case "session.close":
            cmd.closeActiveSession();
            return true;
        case "session.next":
            {
                const releaseModifier = releaseModifierForEvent(event);
                if (releaseModifier) cmd.beginSessionSwitch(1, releaseModifier);
                else cmd.cycleSession(1);
            }
            return true;
        case "session.lastUsed":
            cmd.selectLastSession();
            return true;
        case "session.previous":
            {
                const releaseModifier = releaseModifierForEvent(event);
                if (releaseModifier) cmd.beginSessionSwitch(-1, releaseModifier);
                else cmd.cycleSession(-1);
            }
            return true;
        case "session.nextGroup":
            cmd.cycleSessionGroup(1);
            return true;
        case "agent.permissions":
            if (active?.kind !== "project" || !activeAgentId(st, active)) return false;
            cmd.toggleActiveAgentSkipPermissions();
            return true;
        case "palette.newTab":
            if (st.newTabPaletteOpen) cmd.closeNewTabPalette();
            else cmd.openNewTabPalette();
            return true;
        case "browser.tabNew":
            cmd.newDeskBrowserTab();
            return true;
        case "browser.address":
            return cmd.focusBrowserAddress();
        case "browser.reload":
            if (!reachesBrowser(event)) return false;
            return cmd.reloadBrowserTab();
        case "browser.back":
            if (!reachesBrowser(event)) return false;
            return cmd.browserHistory(-1);
        case "browser.forward":
            if (!reachesBrowser(event)) return false;
            return cmd.browserHistory(1);
        case "window.files":
            cmd.openEditorPane();
            return true;
        case "window.terminal":
            cmd.selectWindowByRole("term");
            return true;
        case "window.git":
            cmd.openGitWorkbench();
            return true;
        case "window.agents":
            cmd.focusAgents();
            return true;
        case "window.search":
            cmd.focusGlobalSearch();
            return true;
    }
    return false;
}

/*
 * Whether the press landed somewhere that types.
 *
 * Asked before an action context is built, because building one walks the
 * session, the window tree and the agent — and a plain letter typed into a
 * terminal is by far the most common keydown there is.
 */
function typingTarget(target: Element | null): boolean {
    if (!target) return false;
    return (
        !!target.closest(".xterm") ||
        target.matches("input, textarea, select") ||
        !!target.closest('[contenteditable="true"], [contenteditable=""], [role="textbox"], .cm-content, [data-takes-keys]')
    );
}

export function useKeymap(): void {
    useEffect(() => {
        const consume = (event: KeyboardEvent): void => {
            event.preventDefault();
            event.stopImmediatePropagation();
        };

        const keydown = (event: KeyboardEvent): void => {
            const target = event.target instanceof Element ? event.target : null;
            if (target?.closest("[data-keybinding-recorder]")) return;

            const st = getState();
            const action = actionForEvent(event, st.keybindingOverrides);

            if (st.sessionSwitcher) {
                if (event.key === "Escape") {
                    cmd.cancelSessionSwitch();
                    consume(event);
                    return;
                }
                if (action === "session.next" || action === "session.previous") {
                    cmd.cycleSessionSwitch(action === "session.next" ? 1 : -1);
                    consume(event);
                    return;
                }
                if (
                    !event.code.startsWith("Alt") &&
                    !event.code.startsWith("Control") &&
                    !event.code.startsWith("Meta") &&
                    !event.code.startsWith("Shift")
                ) {
                    consume(event);
                }
                return;
            }

            if (st.commandPopup && event.key === "Escape") {
                cmd.closeCommandPopup();
                consume(event);
                return;
            }

            // The welcome screen answers the shortcuts it shows, so nothing
            // behind it may, including the ones other modals let through.
            if (st.onboardingOpen) return;
            if (hasOpenModal(st) && !(action && MODAL_ACTIONS.has(action))) return;
            if (st.settingsOpen) {
                if (!action) return;
                if (!KEEPS_SETTINGS.has(action)) cmd.closeSettings();
                if (CLOSES_SETTINGS.has(action)) {
                    consume(event);
                    return;
                }
            }
            // Option belongs to what is being typed into: shells read it as Meta, other layouts type with it.
            if (event.altKey && !event.metaKey && !event.ctrlKey && typingTarget(target)) return;
            if (action && runMeasuredAction(action, "keymap", () => runKeybindingAction(action, event, st))) {
                consume(event);
                return;
            }

            // A built-in that does not apply here leaves its key to the project's own actions.
            if (typingTarget(target) && !event.metaKey && !event.ctrlKey && !event.altKey) return;
            const context = applicationActionContext(st, event.target);
            const contributed = matchApplicationActionKeybinding(event, context);
            if (!contributed) return;
            runMeasuredAction(contributed.commandId, "keymap", () => {
                cmd.noteRecentCommand(`standalone:${contributed.commandId}`);
                void executeApplicationAction(contributed.actionId, context).catch(reportError(`run action ${contributed.commandId}`));
                return true;
            });
            consume(event);
        };

        const keyup = (event: KeyboardEvent): void => {
            const switcher = getState().sessionSwitcher;
            if (!switcher || modifierHeld(event, switcher.releaseModifier)) return;
            cmd.commitSessionSwitch();
            consume(event);
        };

        const commitOnBlur = (): void => {
            if (getState().sessionSwitcher) cmd.commitSessionSwitch();
        };

        /* A chord pressed inside a browser page never reaches this window, so
           the native side hands it over and it replays as a keydown on the
           page's pane, where the same rules apply as for any other pane. */
        const pageChords = new AbortController();
        void browserApi
            .subscribeShortcuts((shortcut) => {
                const pane = document.querySelector<HTMLElement>(`.desk[data-agent-id="${CSS.escape(shortcut.agentId)}"] .browser-viewport`);
                (pane ?? document.body).dispatchEvent(
                    new KeyboardEvent("keydown", {
                        key: shortcut.key,
                        code: shortcut.code,
                        metaKey: true,
                        shiftKey: shortcut.shift,
                        altKey: shortcut.alt,
                        bubbles: true,
                        cancelable: true,
                    }),
                );
            }, pageChords.signal)
            .catch(() => {});

        window.addEventListener("keydown", keydown, { capture: true });
        window.addEventListener("keyup", keyup, { capture: true });
        window.addEventListener("blur", commitOnBlur);
        return () => {
            pageChords.abort();
            window.removeEventListener("keydown", keydown, { capture: true });
            window.removeEventListener("keyup", keyup, { capture: true });
            window.removeEventListener("blur", commitOnBlur);
        };
    }, []);
}
