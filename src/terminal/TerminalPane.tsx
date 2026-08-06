import { useEffect, useRef, useState } from "react";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";
import { usePty } from "./usePty";
import { useXterm } from "./useXterm";
import type { TerminalSearchOptions } from "./interactions";
import { TerminalFindBar } from "./TerminalFindBar";
import { TerminalContextMenu } from "./TerminalContextMenu";
import type { PtyContext } from "../state/types";

const SWITCH_KEEPALIVE_MS = 30_000;
const MAX_HIDDEN_RENDERERS = 4;
const hiddenRendererEvictions = new Map<symbol, () => void>();

function enforceHiddenRendererBudget() {
    while (hiddenRendererEvictions.size > MAX_HIDDEN_RENDERERS) {
        const oldest = hiddenRendererEvictions.values().next().value as (() => void) | undefined;
        if (!oldest) return;
        oldest();
    }
}

export function TerminalPane({
    cwd,
    startup,
    active,
    visible = active,
    spawnWhen = visible,
    context,
    onTitleChange,
}: {
    cwd?: string;
    startup?: string;
    active: boolean;
    visible?: boolean;
    spawnWhen?: boolean;
    context?: PtyContext;
    onTitleChange?: (title: string) => void;
}) {
    const [shouldMount, setShouldMount] = useState(visible);
    const [findOpen, setFindOpen] = useState(false);
    const [findQuery, setFindQuery] = useState("");
    const [findOptions, setFindOptions] = useState<TerminalSearchOptions>({ caseSensitive: false, regex: false, wholeWord: false });
    const [findResult, setFindResult] = useState<ISearchResultChangeEvent>({ resultIndex: -1, resultCount: 0 });
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const hostRef = useRef<HTMLDivElement>(null);
    const rendererTokenRef = useRef(Symbol("terminal-renderer"));
    const ptyReady = usePty({ cwd, startup, hostRef, spawnWhen, context });

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
        const id = window.setTimeout(evict, SWITCH_KEEPALIVE_MS);
        return () => {
            window.clearTimeout(id);
            hiddenRendererEvictions.delete(token);
        };
    }, [visible, shouldMount]);

    const controller = useXterm({
        hostRef,
        ptyReady,
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
    });

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
