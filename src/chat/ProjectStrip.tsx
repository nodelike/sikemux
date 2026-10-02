import { useEffect, useId, useMemo, useRef, useState } from "react";
import { fuzzyScore, isSubstringMatch } from "../lib/fuzzy";
import { leavingMenu } from "../lib/motion";
import { basename, prettyPath } from "../lib/paths";
import * as cmd from "../state/commands";
import { useResource, useResourceEnabled } from "../state/resources";
import { gitOverviewR, projectRootsScanR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { IconChevron, IconFolder, IconGit } from "../ui/Icons";
import { WorktreeToggle } from "./ChatWorktree";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import "../styles/chat/project-strip.css";

interface Choice {
    path: string;
    name: string;
    sub: string;
    open: boolean;
}

/** Keeps a long path's last folders on screen when it is cut short from the start. */
const ltr = (text: string) => `‎${text}‎`;

function ProjectSwitcher({ agentId, cwd, onClose }: { agentId: string; cwd: string; onClose: () => void }) {
    const home = useStore((s) => s.home);
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const projectRoots = useStore((s) => s.projectRoots);
    const scanned = useResourceEnabled(projectRoots.length > 0, projectRootsScanR, projectRoots).data;
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const listId = useId();
    const search = useRef<HTMLInputElement>(null);

    useEffect(() => search.current?.focus(), []);

    const choices = useMemo(() => {
        const open: Choice[] = sessionOrder
            .map((id) => sessionsById[id])
            .filter((session) => session?.kind === "project")
            .map((session) => ({ path: session.cwd, name: basename(session.cwd) || session.name, sub: prettyPath(session.cwd, home), open: true }));
        const opened = new Set(open.map((choice) => choice.path));
        const found: Choice[] = (scanned ?? [])
            .filter((entry) => !opened.has(entry.path))
            .map((entry) => ({ path: entry.path, name: basename(entry.path), sub: prettyPath(entry.path, home), open: false }));
        const scored = [...open, ...found]
            .map((choice) => ({ choice, score: fuzzyScore(query, `${choice.name} ${choice.sub}`) }))
            .filter((row) => row.score >= 0);
        const exact = scored.some((row) => isSubstringMatch(row.score));
        const kept = exact ? scored.filter((row) => isSubstringMatch(row.score)) : scored;
        if (query.trim()) kept.sort((a, b) => a.score - b.score);
        return kept.map((row) => row.choice);
    }, [sessionOrder, sessionsById, scanned, query, home]);

    const pick = (choice: Choice | undefined) => {
        if (!choice) return;
        onClose();
        if (choice.path !== cwd) cmd.moveAgentToProject(agentId, choice.path);
    };

    const grouped = !query.trim();
    return (
        <div ref={leavingMenu} className="chat-picker-menu compact project-switcher">
            <input
                ref={search}
                value={query}
                aria-label="Search projects"
                placeholder="Search projects…"
                role="combobox"
                aria-controls={listId}
                aria-expanded
                aria-autocomplete="list"
                aria-activedescendant={choices[selected] ? `${listId}-${selected}` : undefined}
                onChange={(event) => {
                    setQuery(event.target.value);
                    setSelected(0);
                }}
                onKeyDown={(event) => {
                    if (event.key === "Escape") {
                        event.preventDefault();
                        event.stopPropagation();
                        onClose();
                    }
                    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                        event.preventDefault();
                        setSelected((index) =>
                            choices.length ? (index + (event.key === "ArrowDown" ? 1 : choices.length - 1)) % choices.length : 0,
                        );
                    }
                    if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                        event.preventDefault();
                        pick(choices[selected]);
                    }
                }}
            />
            <div id={listId} role="listbox" aria-label="Projects" className="chat-picker-options">
                {choices.map((choice, index) => (
                    <div key={choice.path} role="presentation">
                        {grouped && (index === 0 || choices[index - 1].open !== choice.open) && (
                            <div className="project-switcher-group" role="presentation">
                                {choice.open ? "Open" : "Found on disk"}
                            </div>
                        )}
                        <button
                            type="button"
                            id={`${listId}-${index}`}
                            role="option"
                            aria-selected={choice.path === cwd}
                            className={`${index === selected ? "highlighted" : ""}${choice.open ? " open" : ""}`}
                            onMouseEnter={() => setSelected(index)}
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => pick(choice)}>
                            <IconFolder size={12} />
                            <strong>{choice.name}</strong>
                            <small>{ltr(choice.sub)}</small>
                        </button>
                    </div>
                ))}
                {choices.length === 0 && <div className="chat-picker-hint">No matches</div>}
            </div>
        </div>
    );
}

/**
 * Where a new chat will run, docked above its composer until the first
 * message. The project name opens a switcher that moves the chat elsewhere.
 */
export function ProjectStrip({
    agentId,
    cwd,
    worktree,
}: {
    agentId: string;
    cwd: string;
    worktree: { state: WorktreeSwitchState; toggle: () => void };
}) {
    const home = useStore((s) => s.home);
    const branch = useResource(gitOverviewR, cwd).data?.status?.branch;
    const [open, setOpen] = useState(false);
    const root = useRef<HTMLDivElement>(null);
    const trigger = useRef<HTMLButtonElement>(null);

    useEffect(() => {
        if (!open) return;
        const outside = (event: PointerEvent) => {
            if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
        };
        document.addEventListener("pointerdown", outside);
        return () => document.removeEventListener("pointerdown", outside);
    }, [open]);

    return (
        <div className="chat-project-strip">
            <div
                className="chat-project"
                ref={root}
                onBlur={(event) => {
                    // WebKit hands focus to nobody when a button is clicked, so a blur
                    // with no new target is a click on our own menu, not a click away.
                    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
                }}>
                <button
                    ref={trigger}
                    type="button"
                    className="chat-project-trigger"
                    aria-label="Project"
                    aria-haspopup="listbox"
                    aria-expanded={open}
                    onClick={() => setOpen(!open)}>
                    <IconFolder size={12} />
                    <span>{basename(cwd) || cwd}</span>
                    <IconChevron size={10} />
                </button>
                {open && (
                    <ProjectSwitcher
                        agentId={agentId}
                        cwd={cwd}
                        onClose={() => {
                            setOpen(false);
                            trigger.current?.focus();
                        }}
                    />
                )}
            </div>
            <span className="chat-project-path" title={cwd}>
                {ltr(prettyPath(cwd, home))}
            </span>
            {branch && (
                <span className="chat-project-branch">
                    <IconGit size={11} />
                    {branch}
                </span>
            )}
            {worktree.state.kind === "choosing" && (
                <>
                    <span className="chat-project-divider" aria-hidden="true" />
                    <WorktreeToggle state={worktree.state} onToggle={worktree.toggle} />
                </>
            )}
        </div>
    );
}
