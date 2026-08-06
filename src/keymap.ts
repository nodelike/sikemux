import { useEffect } from "react";
import { actionForEvent, type KeybindingActionId } from "./keybindings";
import * as cmd from "./state/commands";
import { emit } from "./state/bus";
import { getState, type StoreState } from "./state/store";
import type { KeyModifier } from "./state/types";

function isTerminalKeyTarget(e: KeyboardEvent): boolean {
    const target = e.target instanceof Element ? e.target : document.activeElement;
    return !!target?.closest?.(".xterm");
}

function hasOpenModal(st: StoreState): boolean {
    return (
        st.pickerOpen ||
        st.filePaletteOpen ||
        st.agentPaletteOpen ||
        st.rundeckJobPaletteOpen ||
        st.brunoReqPaletteOpen ||
        st.brunoEnvPaletteOpen ||
        st.commandPaletteOpen ||
        st.commandPopup !== null ||
        st.onboardingOpen ||
        st.diagnosticsOpen ||
        st.whatsNewOpen ||
        st.settingsOpen ||
        st.awsAuthModal !== null
    );
}

const MODAL_ACTIONS = new Set<KeybindingActionId>(["palette.commands", "palette.files", "search.global", "settings.toggle"]);

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

    switch (action) {
        case "palette.commands":
            cmd.toggleCommandPalette();
            return true;
        case "palette.files":
            if (active?.kind === "rundeck") {
                if (st.rundeckJobPaletteOpen) cmd.closeRundeckJobPalette();
                else cmd.openRundeckJobPalette();
            } else if (active?.kind === "bruno") {
                if (st.brunoReqPaletteOpen) cmd.closeBrunoReqPalette();
                else cmd.openBrunoReqPalette();
            } else if (st.filePaletteOpen) {
                cmd.closeFilePalette();
            } else {
                cmd.openFilePalette();
            }
            return true;
        case "search.global": {
            const selection = window.getSelection()?.toString() ?? "";
            cmd.focusGlobalSearch(selection.trim() ? selection : undefined);
            return true;
        }
        case "settings.toggle":
            cmd.toggleSettings();
            return true;
        case "bruno.save":
            if (active?.kind !== "bruno") return false;
            cmd.brunoSaveActive();
            return true;
        case "bruno.send":
            if (active?.kind !== "bruno") return false;
            emit({ type: "bruno-run", sessionId: active.id });
            return true;
        case "pane.splitRow":
            cmd.splitActivePane("row");
            return true;
        case "pane.splitColumn":
            cmd.splitActivePane("column");
            return true;
        case "pane.focusLeft":
            cmd.moveFocus("left");
            return true;
        case "pane.focusDown":
            // Alt+J is a useful multiline fallback in terminal apps. Preserve that
            // physical default while allowing any reassigned focus shortcut through.
            if (event.altKey && !event.metaKey && !event.ctrlKey && !event.shiftKey && event.code === "KeyJ" && isTerminalKeyTarget(event)) {
                return false;
            }
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
        case "pane.close":
            cmd.closeActiveFocusTarget();
            return true;
        case "session.newContextual":
            if (active?.kind === "project" && active.view === "agent") cmd.openAgentPalette();
            else if (active?.kind === "project") cmd.newWindow();
            else if (active?.kind === "command") cmd.createCommandSession();
            else if (active?.kind === "ssh") cmd.openPicker("ssh");
            else if (active?.kind === "aws") cmd.openAwsSession();
            else if (active?.kind === "rundeck") cmd.openRundeckSession();
            else if (active?.kind === "bruno") cmd.openPicker("bruno");
            else return false;
            return true;
        case "window.next":
            cmd.selectWindowRelative(1);
            return true;
        case "window.previous":
            cmd.selectWindowRelative(-1);
            return true;
        case "tab.next":
            cmd.cycleTabs(1);
            return true;
        case "tab.previous":
            cmd.cycleTabs(-1);
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
        case "aws.open":
            cmd.openAwsSession();
            return true;
        case "bruno.open":
            cmd.openPicker("bruno");
            return true;
        case "session.command":
            cmd.focusCommandSession();
            return true;
        case "bruno.environment":
            if (active?.kind !== "bruno") return false;
            cmd.openBrunoEnvPalette();
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
            if (active?.kind !== "project" || active.view !== "agent") return false;
            cmd.toggleActiveAgentSkipPermissions();
            return true;
        case "window.files":
            cmd.selectWindowByRole("files");
            return true;
        case "window.terminal":
            cmd.selectWindowByRole("term");
            return true;
        case "window.git":
            cmd.selectWindowByRole("git");
            return true;
        case "window.agents":
            cmd.focusAgents();
            return true;
        case "window.search":
            cmd.selectWindowByName("search");
            return true;
    }
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

            if (!action) return;
            if (hasOpenModal(st) && !MODAL_ACTIONS.has(action)) return;
            if (!runKeybindingAction(action, event, st)) return;

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

        window.addEventListener("keydown", keydown, { capture: true });
        window.addEventListener("keyup", keyup, { capture: true });
        window.addEventListener("blur", commitOnBlur);
        return () => {
            window.removeEventListener("keydown", keydown, { capture: true });
            window.removeEventListener("keyup", keyup, { capture: true });
            window.removeEventListener("blur", commitOnBlur);
        };
    }, []);
}
