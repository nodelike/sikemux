import { useModalFocus } from "../hooks/useModalFocus";
import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionKind } from "../state/types";
import { fuzzyScore, isSubstringMatch } from "../lib/fuzzy";
import { basename, expandHome, normalizePath, prettyPath, relativePath } from "../lib/paths";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import { settingsApi } from "../api/settings";
import * as cmd from "../state/commands";
import { useResourceEnabled } from "../state/resources";
import { projectRootsScanR, sshHostsR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import type { SshHost } from "../api/ssh";
import { useMouseActive } from "../hooks/useMouseActive";
import { IconBruno, IconClose, IconCommand, IconFolder, IconSearch } from "./Icons";

type Item =
    | { kind: "session"; id: string; name: string; sub: string; sk: SessionKind }
    | { kind: "dir"; path: string; name: string; sub: string }
    | { kind: "bruno"; path: string; name: string; sub: string }
    | { kind: "ssh"; alias: string; name: string; sub: string };

function sshSubtitle(h: SshHost): string {
    const target = h.hostname ?? h.alias;
    const user = h.user ? `${h.user}@` : "";
    const port = h.port && h.port !== 22 ? `:${h.port}` : "";
    return `${user}${target}${port}`;
}

export function SeshPicker() {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = sessionOrder.map((id) => sessionsById[id]);
    const home = useStore((s) => s.home);
    const projectRoots = useStore((s) => s.projectRoots);
    const brunoWorkspaces = useStore((s) => s.brunoWorkspaces);
    const mode = useStore((s) => s.pickerMode);

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const mouseActive = useMouseActive();

    const showProjects = mode === "all" || mode === "projects";
    const showSsh = mode === "all" || mode === "ssh";
    const showBruno = mode === "all" || mode === "bruno";

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const hasConfiguredProjects = projectRoots.length > 0;
    const scanned = useResourceEnabled(showProjects && hasConfiguredProjects, projectRootsScanR, showProjects ? projectRoots : []);
    const hostsR = useResourceEnabled(showSsh, sshHostsR);
    const projects = showProjects ? (scanned.data ?? []) : [];
    const hosts = showSsh ? (hostsR.data ?? []) : [];

    const pretty = (p: string) => prettyPath(p, home);
    const projectLabel = (cwd: string, fallback: string) => {
        const roots = projectRoots
            .map((r) => normalizePath(expandHome(r.path, home)))
            .filter(Boolean)
            .sort((a, b) => b.length - a.length);
        for (const root of roots) {
            if (relativePath(cwd, root) === "") return fallback;
            if (relativePath(cwd, root) !== null) return basename(cwd);
        }
        return fallback;
    };

    const items = useMemo<Item[]>(() => {
        const wantKind = (k: SessionKind) =>
            mode === "all" || (mode === "projects" && k === "project") || (mode === "ssh" && k === "ssh") || (mode === "bruno" && k === "bruno");
        const sessionItems: Item[] = sessions
            .filter((s) => wantKind(s.kind))
            .map((s) => ({
                kind: "session",
                id: s.id,
                name: s.kind === "project" ? projectLabel(s.cwd, s.name) : s.name,
                sub: s.kind === "project" || s.kind === "bruno" ? pretty(s.cwd) : s.kind === "ssh" ? "ssh" : "command",
                sk: s.kind,
            }));

        const openCwds = new Set(sessions.map((s) => s.cwd).filter(Boolean));
        const dirItems: Item[] = showProjects
            ? projects
                  .filter((p) => !openCwds.has(p.path))
                  .map<Item>((p) => ({
                      kind: "dir",
                      path: p.path,
                      name: basename(p.path),
                      sub: pretty(p.path),
                  }))
            : [];

        const openBrunoPaths = new Set(
            sessions
                .filter((s) => s.kind === "bruno")
                .map((s) => s.cwd)
                .filter(Boolean),
        );
        const brunoItems: Item[] = showBruno
            ? brunoWorkspaces
                  .filter((p) => !openBrunoPaths.has(p))
                  .map<Item>((p) => ({
                      kind: "bruno",
                      path: p,
                      name: basename(p),
                      sub: pretty(p),
                  }))
            : [];

        const openSshAliases = new Set(sessions.filter((s) => s.kind === "ssh").map((s) => s.name));
        const sshItems: Item[] = showSsh
            ? hosts
                  .filter((h) => !openSshAliases.has(h.alias))
                  .map<Item>((h) => ({
                      kind: "ssh",
                      alias: h.alias,
                      name: h.alias,
                      sub: sshSubtitle(h),
                  }))
            : [];

        const score = (it: Item) => fuzzyScore(query, `${it.name} ${it.sub}`);
        const scoreGroup = (arr: Item[]) => arr.map((it) => ({ it, s: score(it) })).filter((x) => x.s >= 0);
        const groups = [scoreGroup(sessionItems), scoreGroup(dirItems), scoreGroup(brunoItems), scoreGroup(sshItems)];
        const hasSubstring = groups.some((g) => g.some((x) => isSubstringMatch(x.s)));
        const finalize = (g: { it: Item; s: number }[]) => {
            const kept = hasSubstring ? g.filter((x) => isSubstringMatch(x.s)) : g;
            if (query.trim()) kept.sort((a, b) => a.s - b.s);
            return kept.map((x) => x.it);
        };
        return groups.flatMap(finalize);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessions, projects, hosts, brunoWorkspaces, query, home, mode, projectRoots]);

    useEffect(() => {
        setSel((s) => Math.min(s, Math.max(0, items.length - 1)));
    }, [items.length]);

    const activate = (it: Item | undefined) => {
        if (!it) return;
        if (it.kind === "session") cmd.selectSession(it.id);
        else if (it.kind === "dir") cmd.createProjectSession(it.path);
        else if (it.kind === "bruno") cmd.openBrunoSession(it.path);
        else cmd.createSshSession(it.alias);
    };

    const openFolder = async () => {
        try {
            const picked = await settingsApi.pickFolder(home || undefined);
            if (!picked) return;
            cmd.addProjectRoot(picked, 0, true);
            cmd.createProjectSession(picked);
        } catch (err) {
            reportError("open folder")(err);
        }
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            cmd.closePicker();
        } else if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s + 1) % items.length : 0));
        } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s - 1 + items.length) % items.length : 0));
        } else if (e.key === "Enter") {
            e.preventDefault();
            activate(items[sel]);
        }
    };

    const firstDirIdx = items.findIndex((it) => it.kind === "dir");
    const firstBrunoIdx = items.findIndex((it) => it.kind === "bruno");
    const firstSshIdx = items.findIndex((it) => it.kind === "ssh");
    const firstSessIdx = items.findIndex((it) => it.kind === "session");

    const placeholder =
        mode === "projects"
            ? "find a project…"
            : mode === "ssh"
              ? "ssh — search hosts from ~/.ssh/config…"
              : mode === "bruno"
                ? "open or pick a bruno workspace…"
                : "jump to a session, project, or ssh host…";

    return (
        <div className="picker-backdrop" onMouseDown={cmd.closePicker}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker"
                role="dialog"
                aria-modal="true"
                aria-label="Open session or project"
                onMouseDown={(e) => e.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        placeholder={placeholder}
                        value={query}
                        onChange={(e) => {
                            setQuery(e.target.value);
                            setSel(0);
                        }}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                    {showProjects && (
                        <button className="picker-folder-btn" onClick={() => void openFolder()} title="Open folder" type="button">
                            <IconFolder size={14} />
                        </button>
                    )}
                    {mode === "bruno" && (
                        <button
                            className="picker-folder-btn"
                            onClick={() => void cmd.openBrunoFolder()}
                            title="Import a Bruno workspace folder"
                            type="button">
                            <IconFolder size={14} />
                        </button>
                    )}
                </div>

                <div className="picker-list">
                    {items.length === 0 && (
                        <div className="picker-empty">
                            {showProjects && !hasConfiguredProjects ? (
                                <>
                                    no projects configured —{" "}
                                    <button
                                        className="picker-link"
                                        onClick={() => {
                                            cmd.closePicker();
                                            cmd.openSettings();
                                        }}>
                                        open settings
                                    </button>{" "}
                                    to add some ({PRIMARY_SHORTCUT},)
                                </>
                            ) : showSsh && hosts.length === 0 && mode === "ssh" ? (
                                "no hosts in ~/.ssh/config"
                            ) : mode === "bruno" && brunoWorkspaces.length === 0 ? (
                                <button className="picker-link" onClick={() => void cmd.openBrunoFolder()}>
                                    import a Bruno workspace folder
                                </button>
                            ) : (
                                "no matches"
                            )}
                        </div>
                    )}
                    {items.map((it, i) => {
                        const sessLabel = i === firstSessIdx && firstSessIdx >= 0 ? "Open" : null;
                        const dirLabel = i === firstDirIdx && firstDirIdx >= 0 ? "Projects" : null;
                        const brunoLabel = i === firstBrunoIdx && firstBrunoIdx >= 0 ? "API" : null;
                        const sshLabel = i === firstSshIdx && firstSshIdx >= 0 ? "SSH" : null;
                        const key =
                            it.kind === "session"
                                ? `s-${it.id}`
                                : it.kind === "dir"
                                  ? `d-${it.path}`
                                  : it.kind === "bruno"
                                    ? `b-${it.path}`
                                    : `h-${it.alias}`;
                        const isBruno = it.kind === "bruno" || (it.kind === "session" && it.sk === "bruno");
                        const isFolder = it.kind === "dir" || (it.kind === "session" && it.sk === "project");
                        const iconClass = isBruno ? "bruno" : it.kind === "dir" ? "project" : it.kind === "session" ? it.sk : "command";
                        return (
                            <div key={key}>
                                {sessLabel && <div className="picker-group">{sessLabel}</div>}
                                {dirLabel && <div className="picker-group">{dirLabel}</div>}
                                {brunoLabel && <div className="picker-group">{brunoLabel}</div>}
                                {sshLabel && <div className="picker-group">{sshLabel}</div>}
                                <div
                                    className="picker-item-wrap"
                                    onMouseEnter={() => {
                                        if (mouseActive.current) setSel(i);
                                    }}>
                                    <button className={`picker-item${i === sel ? " sel" : ""}`} onClick={() => activate(it)}>
                                        <span className={`picker-icon ${iconClass}`}>
                                            {isBruno ? <IconBruno size={14} /> : isFolder ? <IconFolder size={14} /> : <IconCommand size={14} />}
                                        </span>
                                        <span className="picker-name">{it.name}</span>
                                        <span className="picker-sub">{it.sub}</span>
                                    </button>
                                    {it.kind === "bruno" && (
                                        <button
                                            type="button"
                                            className="picker-forget"
                                            aria-label={`Forget ${it.name} workspace`}
                                            title="Forget this workspace"
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                cmd.removeBrunoWorkspace(it.path);
                                            }}>
                                            <IconClose size={11} />
                                        </button>
                                    )}
                                </div>
                            </div>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
