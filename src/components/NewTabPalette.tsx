import { useEffect, useRef, useState, type ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { useModalFocus } from "../hooks/useModalFocus";
import { IconAgent, IconCommit, IconEditor, IconGlobe, IconRun, IconSearch } from "./Icons";

interface TabChoice {
    id: string;
    label: string;
    detail: string;
    icon: ReactNode;
    open: () => void;
    disabled?: boolean;
}

export function NewTabPalette() {
    const session = useStore((state) => state.sessions[state.activeSessionId]);
    const agentIds = useStore((state) => state.agentsBySession[state.activeSessionId]);
    const agents = useStore((state) => state.agents);
    const project = session?.kind === "project";
    const browserAgent = (agentIds ?? []).map((id) => agents[id]).find(Boolean);
    const [selected, setSelected] = useState(0);
    const selectedRef = useRef(0);
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const choices: TabChoice[] = [
        {
            id: "terminal",
            label: "Terminal",
            detail: "A new shell in this project",
            icon: <IconRun size={14} />,
            open: cmd.newWindow,
            disabled: !session,
        },
        {
            id: "agent",
            label: "Agent",
            detail: project ? "Start or resume an agent" : "Open a project first",
            icon: <IconAgent size={14} />,
            open: cmd.openAgentPalette,
            disabled: !project,
        },
        {
            id: "browser",
            label: "Browser",
            detail: browserAgent ? `Browse alongside ${browserAgent.title}` : "Start an agent to use its browser",
            icon: <IconGlobe size={14} />,
            open: () => {
                void cmd.newBrowserTab();
            },
            disabled: !browserAgent,
        },
        {
            id: "editor",
            label: "Editor",
            detail: project ? "Open a project file" : "Open a project first",
            icon: <IconEditor size={14} />,
            open: cmd.openFilePalette,
            disabled: !project,
        },
        {
            id: "git",
            label: "Git",
            detail: project ? "Changes, branches, remotes and stashes" : "Open a project first",
            icon: <IconCommit size={14} />,
            open: cmd.openGitWorkbench,
            disabled: !project,
        },
        {
            id: "search",
            label: "Search",
            detail: project ? "Search across the project" : "Open a project first",
            icon: <IconSearch size={14} />,
            open: cmd.focusGlobalSearch,
            disabled: !project,
        },
    ];
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
        <div className="picker-backdrop" onMouseDown={cmd.closeNewTabPalette}>
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
                            onClick={() => choose(choice)}>
                            <span className="picker-icon">{choice.icon}</span>
                            <span className="picker-text">
                                <span className="picker-name">{choice.label}</span> <span className="picker-sub">{choice.detail}</span>
                            </span>
                            <kbd className="new-tab-key">{index + 1}</kbd>
                        </button>
                    ))}
                </div>
                <div className="picker-footer">Esc to cancel</div>
            </div>
        </div>
    );
}
