import { useCallback, useEffect, useRef, useState } from "react";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";
import { usePty } from "./usePty";
import { useXterm } from "./useXterm";
import type { TerminalSearchOptions } from "./interactions";
import { TerminalFindBar } from "./TerminalFindBar";
import { TerminalContextMenu } from "./TerminalContextMenu";
import type { PtyContext, PtyDirectCommand } from "../state/types";
import { applyPtyShellMetadataEvent, type PtyShellMetadataEvent } from "../api/ptyShell";
import type { PtyShellMetadataSnapshot } from "./ptyController";
import { basename } from "../lib/paths";

export const TERMINAL_RENDERER_RETENTION = Object.freeze({ keepaliveMs: 15_000, maxHidden: 3 });
const hiddenRendererEvictions = new Map<symbol, () => void>();

function enforceHiddenRendererBudget() {
    while (hiddenRendererEvictions.size > TERMINAL_RENDERER_RETENTION.maxHidden) {
        const oldest = hiddenRendererEvictions.values().next().value as (() => void) | undefined;
        if (!oldest) return;
        oldest();
    }
}

export function TerminalPane({
    cwd,
    startup,
    directCommand,
    initialDropPaths,
    initialInput,
    onInitialInputDelivered,
    active,
    visible = active,
    spawnWhen = visible,
    context,
    externallyOwned = false,
    retainPtyOnUnmount = false,
    onTitleChange,
    onExit,
}: {
    cwd?: string;
    startup?: string;
    directCommand?: PtyDirectCommand;
    /** Native file drops replayed before the first submitted task. */
    initialDropPaths?: readonly string[];
    /** First submitted task for interactive CLIs without an argv prompt. */
    initialInput?: string;
    onInitialInputDelivered?: () => void;
    active: boolean;
    visible?: boolean;
    spawnWhen?: boolean;
    context?: PtyContext;
    /** Borrow a task-owned PTY bound to this pane instead of spawning a shell. */
    externallyOwned?: boolean;
    /** Item controllers set this; transient/embedded terminals remain local. */
    retainPtyOnUnmount?: boolean;
    onTitleChange?: (title: string) => void;
    /** Fires when the shell process ends. Remount with a fresh key to respawn. */
    onExit?: () => void;
}) {
    const [shouldMount, setShouldMount] = useState(visible);
    const [findOpen, setFindOpen] = useState(false);
    const [findQuery, setFindQuery] = useState("");
    const [findOptions, setFindOptions] = useState<TerminalSearchOptions>({ caseSensitive: false, regex: false, wholeWord: false });
    const [findResult, setFindResult] = useState<ISearchResultChangeEvent>({ resultIndex: -1, resultCount: 0 });
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [shellMetadata, setShellMetadata] = useState<PtyShellMetadataSnapshot | null>(null);
    const hostRef = useRef<HTMLDivElement>(null);
    const rendererTokenRef = useRef(Symbol("terminal-renderer"));
    const titleChangeRef = useRef(onTitleChange);
    titleChangeRef.current = onTitleChange;
    const shellIntegration = !externallyOwned && context?.shellIntegration === true;
    const applyShellEvent = useCallback((event: PtyShellMetadataEvent) => {
        setShellMetadata((current) => applyPtyShellMetadataEvent(current, event));
    }, []);
    const applyShellSnapshot = useCallback((metadata: PtyShellMetadataSnapshot | null) => {
        setShellMetadata((current) => {
            if (!metadata) return null;
            return current && current.revision > metadata.revision ? current : metadata;
        });
    }, []);
    const ptyController = usePty({
        cwd,
        startup,
        directCommand,
        initialDropPaths,
        initialInput,
        onInitialInputDelivered,
        hostRef,
        spawnWhen,
        context,
        externallyOwned,
        onShellMetadata: shellIntegration ? applyShellEvent : undefined,
        durableItemId: retainPtyOnUnmount ? context?.paneId : undefined,
    });

    useEffect(() => {
        if (!shellIntegration) setShellMetadata(null);
    }, [shellIntegration]);

    useEffect(() => {
        if (!shellIntegration || !shellMetadata?.cwd) return;
        titleChangeRef.current?.(basename(shellMetadata.cwd));
    }, [shellIntegration, shellMetadata?.cwd]);

    useEffect(() => {
        const token = rendererTokenRef.current;
        const evict = () => {
            hiddenRendererEvictions.delete(token);
            setShouldMount(false);
        };
        if (visible) {
            hiddenRendererEvictions.delete(token);
            setShouldMount(true);
            return;
        }
        if (!shouldMount) return;
        hiddenRendererEvictions.delete(token);
        hiddenRendererEvictions.set(token, evict);
        enforceHiddenRendererBudget();
        const id = window.setTimeout(evict, TERMINAL_RENDERER_RETENTION.keepaliveMs);
        return () => {
            window.clearTimeout(id);
            hiddenRendererEvictions.delete(token);
        };
    }, [visible, shouldMount]);

    const controller = useXterm({
        hostRef,
        ptyController,
        shouldMount,
        active,
        visible,
        onFindRequest: (seed) => {
            if (seed) setFindQuery(seed);
            setMenu(null);
            setFindOpen(true);
        },
        onSearchResults: setFindResult,
        onTitleChange,
        onShellMetadata: shellIntegration ? applyShellSnapshot : undefined,
        onExit,
    });

    const shellPhaseLabel =
        shellMetadata?.phase === "finished" && shellMetadata.lastExitCode !== null
            ? shellMetadata.lastExitCode === 0
                ? "done"
                : `exit ${shellMetadata.lastExitCode}`
            : shellMetadata?.phase;
    const shellDirectoryLabel = shellMetadata?.cwd ? basename(shellMetadata.cwd) : null;

    useEffect(() => {
        if (visible) return;
        setFindOpen(false);
        setMenu(null);
        controller.clearSearch();
    }, [visible, controller]);

    const closeFind = () => {
        setFindOpen(false);
        controller.clearSearch();
        window.requestAnimationFrame(() => controller.focus());
    };

    const openFind = (seed: string) => {
        if (seed) setFindQuery(seed);
        setMenu(null);
        setFindOpen(true);
    };

    return (
        <div
            className="terminal-shell"
            onContextMenu={(event) => {
                event.preventDefault();
                setMenu({ x: event.clientX, y: event.clientY });
            }}>
            <div ref={hostRef} className="terminal-host" />
            {shellIntegration && shellMetadata && (
                <div
                    className="terminal-shell-metadata"
                    data-phase={shellMetadata.phase}
                    title={shellMetadata.cwd ? `Shell-reported directory: ${shellMetadata.cwd}` : "Shell-reported command state"}
                    aria-label={`Shell reported ${shellDirectoryLabel ? `${shellDirectoryLabel}, ` : ""}${shellPhaseLabel ?? "unknown"}`}>
                    <span className="terminal-shell-metadata-dot" aria-hidden="true" />
                    {shellDirectoryLabel && <span className="terminal-shell-metadata-cwd">{shellDirectoryLabel}</span>}
                    {shellPhaseLabel && <span className="terminal-shell-metadata-phase">{shellPhaseLabel}</span>}
                </div>
            )}
            {findOpen && (
                <TerminalFindBar
                    controller={controller}
                    query={findQuery}
                    onQueryChange={setFindQuery}
                    options={findOptions}
                    onOptionsChange={setFindOptions}
                    result={findResult}
                    onClose={closeFind}
                />
            )}
            {menu && <TerminalContextMenu x={menu.x} y={menu.y} controller={controller} onFind={openFind} onClose={() => setMenu(null)} />}
        </div>
    );
}
