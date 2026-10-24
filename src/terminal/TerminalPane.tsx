import { useCallback, useEffect, useRef, useState } from "react";
import "./terminal.css";
import { usePty } from "./usePty";
import { useTerminal } from "./useTerminal";
import type { SearchResults } from "./view";
import type { TerminalSearchOptions } from "./interactions";
import { TerminalFindBar } from "./TerminalFindBar";
import { TerminalContextMenu } from "./TerminalContextMenu";
import type { PtyContext, PtyDirectCommand } from "../state/types";
import { applyPtyShellMetadataEvent, type PtyShellMetadataEvent } from "../api/ptyShell";
import { ptyCall } from "../api/core";
import type { PtyShellMetadataSnapshot } from "./ptyController";
import { basename } from "../lib/paths";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { getState } from "../state/store";
import { terminalName, terminalSelectionDelivery } from "./selectionDelivery";
import { swallow } from "../state/toast";

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
    reconnects,
    directCommand,
    initialDraft,
    onInitialInputDelivered,
    active,
    visible = active,
    spawnWhen = visible,
    context,
    taskTerminalKey,
    retainPtyOnUnmount = false,
    resumePtyId,
    onPtySession,
    onExit,
    host,
}: {
    cwd?: string;
    startup?: string;
    /** An ssh terminal, which connects again when the pane becomes active and its shell sits at the prompt, as after SSH gave up. */
    reconnects?: boolean;
    directCommand?: PtyDirectCommand;
    /** Pasted at the program's prompt once it starts, and never submitted. */
    initialDraft?: string;
    onInitialInputDelivered?: () => void;
    active: boolean;
    visible?: boolean;
    spawnWhen?: boolean;
    context?: PtyContext;
    /** Show the process of the task run behind this key instead of spawning a shell. */
    taskTerminalKey?: string;
    /** Item controllers set this; transient/embedded terminals remain local. */
    retainPtyOnUnmount?: boolean;
    /** The terminal this pane showed before the app last closed, taken back if it still runs. */
    resumePtyId?: number;
    /** Another host the app joined, which runs the terminal. */
    host?: string;
    /** Hears which terminal this pane shows, so it can be found again after a restart. */
    onPtySession?: (id: number) => void;
    /** Fires when the shell process ends. Remount with a fresh key to respawn. */
    onExit?: () => void;
}) {
    const [shouldMount, setShouldMount] = useState(visible);
    const [findOpen, setFindOpen] = useState(false);
    const [findQuery, setFindQuery] = useState("");
    const [findOptions, setFindOptions] = useState<TerminalSearchOptions>({ caseSensitive: false, regex: false, wholeWord: false });
    const [findResult, setFindResult] = useState<SearchResults>({ resultIndex: -1, resultCount: 0 });
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [sendMenu, setSendMenu] = useState<{ x: number; y: number; selection: string } | null>(null);
    const [shellMetadata, setShellMetadata] = useState<PtyShellMetadataSnapshot | null>(null);
    const hostRef = useRef<HTMLDivElement>(null);
    const rendererTokenRef = useRef(Symbol("terminal-renderer"));
    const shellIntegration = !taskTerminalKey && context?.shellIntegration === true;
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
        initialDraft,
        onInitialInputDelivered,
        hostRef,
        spawnWhen,
        context,
        taskTerminalKey,
        onShellMetadata: shellIntegration ? applyShellEvent : undefined,
        durableItemId: retainPtyOnUnmount ? context?.paneId : undefined,
        resumePtyId,
        onPtySession,
        host,
    });

    useEffect(() => {
        const controller = ptyController.current;
        if (!active || !reconnects || controller?.getSnapshot().status !== "running") return;
        void controller
            .start()
            .then((id) => ptyCall(id, "reconnectSsh"))
            .catch(swallow("reconnect at the prompt"));
    }, [active, reconnects, ptyController]);

    useEffect(() => {
        if (!shellIntegration) setShellMetadata(null);
    }, [shellIntegration]);

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

    const controller = useTerminal({
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
        setSendMenu(null);
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
            {menu && (
                <TerminalContextMenu
                    x={menu.x}
                    y={menu.y}
                    controller={controller}
                    onFind={openFind}
                    onSendSelection={context?.sessionKind === "project" ? (selection) => setSendMenu({ ...menu, selection }) : undefined}
                    onClose={() => setMenu(null)}
                />
            )}
            {sendMenu && (
                <SendToAgentMenu
                    x={sendMenu.x}
                    y={sendMenu.y}
                    sessionId={context?.sessionId}
                    delivery={() => terminalSelectionDelivery(sendMenu.selection, terminalName(getState(), context), shellMetadata?.cwd ?? cwd)}
                    onClose={() => setSendMenu(null)}
                />
            )}
        </div>
    );
}
