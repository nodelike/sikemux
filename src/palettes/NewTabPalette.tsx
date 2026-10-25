import { useEffect, useRef, useState, type ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { nearestAgentId } from "../state/selectors";
import { useShortcutLabel } from "../commands/useShortcutLabel";
import { useModalFocus } from "../hooks/useModalFocus";
import { useMouseActive } from "../hooks/useMouseActive";
import { IconAgent, IconDownload, IconGlobe, IconPhone, IconTerminal } from "@shared/ui/Icons";
import { WindowIcon } from "../ui/WindowIcon";
import { openInstallPane } from "../hosts/hosts";
import { PROJECT_SURFACES } from "../workbench/surfaces";
import { simUsable, useSimStatus } from "../sim/simStatus";
import { leavingOverlay } from "../lib/motion";

interface TabChoice {
    id: string;
    label: string;
    detail: string;
    icon: ReactNode;
    shortcut: string;
    open: () => void;
    disabled?: boolean;
}

export function NewTabPalette() {
    const session = useStore((state) => state.sessions[state.activeSessionId]);
    const browserAgent = useStore((state) => {
        const id = nearestAgentId(state);
        return id ? state.agents[id] : undefined;
    });
    const project = session?.kind === "project";
    const simulatorUsable = simUsable(useSimStatus());
    const shortcuts = {
        terminal: useShortcutLabel("terminal.new"),
        agent: useShortcutLabel("agent.choose"),
        browser: useShortcutLabel("browser.tabNew"),
        file: useShortcutLabel("palette.files"),
        git: useShortcutLabel(PROJECT_SURFACES.git.shortcut),
        search: useShortcutLabel(PROJECT_SURFACES.search.shortcut),
        board: useShortcutLabel(PROJECT_SURFACES.board.shortcut),
    };
    const [selected, setSelected] = useState(0);
    const selectedRef = useRef(0);
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const mouseActive = useMouseActive();
    const sshHost = useStore((state) => (state.chosenHost.kind === "ssh" ? state.chosenHost.alias : null));
    const everyChoice: TabChoice[] = [
        {
            id: "terminal",
            label: "Terminal",
            detail: session?.kind === "ssh" ? `Another login on ${session.name}` : project ? "A new shell in this project" : "A new shell",
            icon: <IconTerminal size={14} />,
            shortcut: shortcuts.terminal,
            open: cmd.newTerminal,
        },
        {
            id: "agent",
            label: "Agent",
            detail: project ? "Start or resume an agent" : "Pick a project, then start an agent there",
            icon: <IconAgent size={14} />,
            shortcut: shortcuts.agent,
            open: cmd.chooseAgent,
        },
        {
            id: "browser",
            label: "Browser",
            detail: browserAgent ? `Browse on ${browserAgent.title}'s desk` : "Start an agent to use its browser",
            icon: <IconGlobe size={14} />,
            shortcut: shortcuts.browser,
            open: cmd.newDeskBrowserTab,
            disabled: !browserAgent,
        },
        ...(simulatorUsable
            ? [
                  {
                      id: "simulator",
                      label: "iOS Simulator",
                      detail: browserAgent ? `Drive an iPhone on ${browserAgent.title}'s desk` : "Start an agent to use its desk",
                      icon: <IconPhone size={14} />,
                      shortcut: "",
                      open: cmd.newDeskSimulator,
                      disabled: !browserAgent,
                  },
              ]
            : []),
        {
            id: "editor",
            label: PROJECT_SURFACES.files.label,
            detail: project ? "Open a project file" : "Open a project first",
            icon: <WindowIcon role="files" size={14} />,
            shortcut: shortcuts.file,
            open: cmd.openFilePalette,
            disabled: !project,
        },
        {
            id: "git",
            label: PROJECT_SURFACES.git.label,
            detail: project ? "Changes, branches, remotes and stashes" : "Open a project first",
            icon: <WindowIcon role="git" size={14} />,
            shortcut: shortcuts.git,
            open: PROJECT_SURFACES.git.open,
            disabled: !project,
        },
        {
            id: "search",
            label: PROJECT_SURFACES.search.label,
            detail: project ? "Search across the project" : "Open a project first",
            icon: <WindowIcon role="search" size={14} />,
            shortcut: shortcuts.search,
            open: PROJECT_SURFACES.search.open,
            disabled: !project,
        },
        {
            id: "board",
            label: PROJECT_SURFACES.board.label,
            detail: project ? "The cards agents and you plan the work on" : "Open a project first",
            icon: <WindowIcon role="board" size={14} />,
            shortcut: shortcuts.board,
            open: PROJECT_SURFACES.board.open,
            disabled: !project,
        },
    ];
    // An SSH host without Sikemux is a shell: a login there, or installing Sikemux (ADR 0019).
    const choices: TabChoice[] = sshHost
        ? [
              everyChoice[0],
              {
                  id: "install",
                  label: "Install Sikemux",
                  detail: `What Sikemux adds on ${sshHost}, and the installer`,
                  icon: <IconDownload size={14} />,
                  shortcut: shortcuts.agent,
                  open: () => openInstallPane(sshHost),
              },
          ]
        : everyChoice;
    const choose = (choice: TabChoice | undefined) => {
        if (!choice || choice.disabled) return;
        cmd.closeNewTabPalette();
        choice.open();
    };
    const move = (direction: number) => {
        let next = selectedRef.current;
        for (let count = 0; count < choices.length; count++) {
            next = (next + direction + choices.length) % choices.length;
            if (!choices[next].disabled) break;
        }
        selectedRef.current = next;
        setSelected(next);
    };
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                cmd.closeNewTabPalette();
            } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                move(event.key === "ArrowDown" ? 1 : -1);
            } else if (event.key === "Enter") {
                event.preventDefault();
                choose(choices[selectedRef.current]);
            } else if (/^[1-6]$/.test(event.key) && !event.metaKey && !event.ctrlKey && !event.altKey) {
                event.preventDefault();
                choose(choices[Number(event.key) - 1]);
            }
        };
        const element = modalRef.current;
        element?.addEventListener("keydown", onKey);
        return () => element?.removeEventListener("keydown", onKey);
    });
    useEffect(() => {
        modalRef.current?.querySelector<HTMLElement>(`[data-choice="${selected}"]`)?.focus();
    }, [selected]);
    return (
        <div ref={leavingOverlay} className="picker-backdrop" onMouseDown={cmd.closeNewTabPalette}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker new-tab-palette"
                role="dialog"
                aria-modal="true"
                aria-label="New tab"
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="new-tab-head">
                    <span>New tab</span>
                    <span className="new-tab-hint">Choose a type or press its number</span>
                </div>
                <div className="picker-list">
                    {choices.map((choice, index) => (
                        <button
                            key={choice.id}
                            type="button"
                            data-choice={index}
                            disabled={choice.disabled}
                            className={`picker-item new-tab-item${selected === index ? " sel" : ""}`}
                            onFocus={() => {
                                selectedRef.current = index;
                                setSelected(index);
                            }}
                            onMouseEnter={() => {
                                if (!mouseActive.current || choice.disabled) return;
                                selectedRef.current = index;
                                setSelected(index);
                            }}
                            onClick={() => choose(choice)}>
                            <span className="new-tab-icon">{choice.icon}</span>
                            <span className="new-tab-label">{choice.label}</span>
                            <span className="new-tab-detail">{choice.detail}</span>
                            {choice.shortcut && <span className="new-tab-shortcut">{choice.shortcut}</span>}
                            <kbd className="new-tab-key">{index + 1}</kbd>
                        </button>
                    ))}
                </div>
                <div className="picker-footer">Esc to cancel</div>
            </div>
        </div>
    );
}
