import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { IconAgent, IconCommit, IconEditor, IconGlobe, IconRun, IconSearch } from "./Icons";

interface TabChoice {
    id: string;
    label: string;
    detail: string;
    icon: ReactNode;
    open: () => void;
}

export function NewTabPalette() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const agentIds = useStore((s) => s.agentsBySession[s.activeSessionId]);
    const agentsById = useStore((s) => s.agents);
    const [selected, setSelected] = useState(0);
    // Two keys pressed inside one frame must not both read the pre-render
    // selection, so the handler moves through a ref and state only mirrors it.
    const selectedRef = useRef(0);
    const listRef = useRef<HTMLDivElement>(null);

    const moveSelection = (next: number) => {
        selectedRef.current = next;
        setSelected(next);
    };

    const isProject = session?.kind === "project";
    const browserAgent = (agentIds ?? []).map((id) => agentsById[id]).find(Boolean);

    const choices = useMemo<TabChoice[]>(() => {
        const all: (TabChoice | null)[] = [
            {
                id: "terminal",
                label: "Terminal",
                detail: "A new shell in this project",
                icon: <IconRun size={14} />,
                open: () => cmd.newWindow(),
            },
            isProject
                ? {
                      id: "agent",
                      label: "Agent",
                      detail: "Pick an agent to start or resume",
                      icon: <IconAgent size={14} />,
                      open: () => cmd.openAgentPalette(),
                  }
                : null,
            browserAgent
                ? {
                      id: "browser",
                      label: "Browser",
                      detail: `Browse alongside ${browserAgent.title}`,
                      icon: <IconGlobe size={14} />,
                      open: () => void cmd.newBrowserTab(),
                  }
                : null,
            isProject
                ? {
                      id: "editor",
                      label: "Editor",
                      detail: "Open the file editor",
                      icon: <IconEditor size={14} />,
                      open: () => cmd.openEditorPane(),
                  }
                : null,
            isProject
                ? {
                      id: "diff",
                      label: "Diff",
                      detail: "Review this project's changes",
                      icon: <IconCommit size={14} />,
                      open: () => cmd.openDiffPane(),
                  }
                : null,
            isProject
                ? {
                      id: "search",
                      label: "Search",
                      detail: "Search across the project",
                      icon: <IconSearch size={14} />,
                      open: () => cmd.focusGlobalSearch(),
                  }
                : null,
        ];
        return all.filter((choice): choice is TabChoice => choice !== null);
    }, [isProject, browserAgent]);

    useEffect(() => {
        moveSelection(Math.min(selectedRef.current, Math.max(0, choices.length - 1)));
    }, [choices.length]);

    const choose = (choice: TabChoice | undefined) => {
        if (!choice) return;
        cmd.closeNewTabPalette();
        choice.open();
    };

    // The digits are the point of this palette: ⌘T then 1 opens a terminal
    // without the hand leaving the keyboard.
    useEffect(() => {
        const onKeyDown = (event: KeyboardEvent) => {
            if (event.key === "Escape") {
                event.preventDefault();
                cmd.closeNewTabPalette();
                return;
            }
            if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
                event.preventDefault();
                moveSelection(choices.length ? (selectedRef.current + 1) % choices.length : 0);
                return;
            }
            if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
                event.preventDefault();
                moveSelection(choices.length ? (selectedRef.current - 1 + choices.length) % choices.length : 0);
                return;
            }
            if (event.key === "Enter") {
                event.preventDefault();
                choose(choices[selectedRef.current]);
                return;
            }
            const digit = Number.parseInt(event.key, 10);
            if (Number.isInteger(digit) && digit >= 1 && digit <= choices.length) {
                event.preventDefault();
                choose(choices[digit - 1]);
            }
        };
        window.addEventListener("keydown", onKeyDown, true);
        return () => window.removeEventListener("keydown", onKeyDown, true);
    }, [choices]);

    useEffect(() => {
        listRef.current?.querySelector<HTMLElement>(".picker-item.sel")?.scrollIntoView({ block: "nearest" });
    }, [selected]);

    return (
        <div className="picker-backdrop" onMouseDown={cmd.closeNewTabPalette}>
            <div className="picker new-tab-palette" onMouseDown={(event) => event.stopPropagation()}>
                <div className="new-tab-head">
                    <span>New tab</span>
                    <span className="new-tab-hint">press a number</span>
                </div>
                <div className="picker-list" ref={listRef}>
                    {choices.map((choice, index) => (
                        <button
                            key={choice.id}
                            type="button"
                            className={`picker-item new-tab-item${index === selected ? " sel" : ""}`}
                            onMouseEnter={() => moveSelection(index)}
                            onClick={() => choose(choice)}>
                            <kbd className="new-tab-key">{index + 1}</kbd>
                            <span className="new-tab-icon">{choice.icon}</span>
                            <span className="new-tab-label">{choice.label}</span>
                            <span className="new-tab-detail">{choice.detail}</span>
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
