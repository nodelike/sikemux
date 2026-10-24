import { useEffect, useRef, type RefObject } from "react";
import { currentTerminalTheme, registerTerminal } from "../themes/bus";
import { currentTerminalFontSize, registerTerminalFontSize } from "./fontSize";
import { stageMoving, whenStageStill } from "../state/nativeViews";
import { performanceTelemetry } from "../lib/performance";
import type { PtyShellMetadataSnapshot } from "./ptyController";
import { RendererRestartBackoff } from "./restartBackoff";
import type { NativePtyController } from "./usePty";
import { swallow } from "../state/toast";
import type { TerminalSearchOptions } from "./interactions";
import { TerminalView, type SearchResults } from "./view";

const FONT = '"JetBrainsMono NF", "JetBrainsMono Nerd Font", monospace';
const FONT_SIZE = 13;
const FONT_WEIGHT = 500;
const FONT_WEIGHT_BOLD = 700;
const RENDERER_STABLE_RESET_MS = 5_000;

/*
 * Starting a terminal lays out the page and draws every row, and a screen that
 * comes on stage starts all of its terminals at once. Doing that while the
 * screen slides in froze the slide, so they start once the stage stops, one a
 * frame, the focused one first.
 */
const waitingToStart: Array<() => void> = [];
let startFrame = 0;

function startNext(): void {
    startFrame = 0;
    waitingToStart.shift()?.();
    if (waitingToStart.length > 0) startFrame = requestAnimationFrame(startNext);
}

function startInTurn(start: () => void, first: boolean): () => void {
    let queued = false;
    const stopWaiting = whenStageStill(() => {
        queued = true;
        if (first) waitingToStart.unshift(start);
        else waitingToStart.push(start);
        if (!startFrame) startFrame = requestAnimationFrame(startNext);
    });
    return () => {
        stopWaiting();
        if (!queued) return;
        const at = waitingToStart.indexOf(start);
        if (at >= 0) waitingToStart.splice(at, 1);
        if (waitingToStart.length === 0 && startFrame) {
            cancelAnimationFrame(startFrame);
            startFrame = 0;
        }
    };
}

function settle(operation: Promise<unknown>): void {
    // PtyLifecycleController records the failure; this only keeps it off window.
    void operation.catch(() => {});
}

function font(size: number) {
    return { family: FONT, size, weight: FONT_WEIGHT, boldWeight: FONT_WEIGHT_BOLD };
}

export interface TerminalController {
    find(query: string, direction: "next" | "previous", options: TerminalSearchOptions, incremental?: boolean): Promise<boolean>;
    clearSearch(): void;
    getSelection(): string;
    copySelection(): Promise<boolean>;
    pasteClipboard(): Promise<boolean>;
    selectAll(): void;
    copyScrollback(): Promise<boolean>;
    clear(): void;
    focus(): void;
}

export function useTerminal(opts: {
    hostRef: RefObject<HTMLDivElement | null>;
    ptyController: RefObject<NativePtyController | null>;
    shouldMount: boolean;
    active: boolean;
    visible: boolean;
    onFindRequest?: (seed: string) => void;
    onSearchResults?: (result: SearchResults) => void;
    onShellMetadata?: (metadata: PtyShellMetadataSnapshot | null) => void;
    onExit?: () => void;
}): TerminalController {
    const { hostRef, ptyController, shouldMount, active, visible } = opts;
    const callbacks = useRef(opts);
    callbacks.current = opts;
    const viewRef = useRef<TerminalView | null>(null);
    const controllerRef = useRef<TerminalController | null>(null);
    if (!controllerRef.current) {
        const none = () => callbacks.current.onSearchResults?.({ resultIndex: -1, resultCount: 0 });
        controllerRef.current = {
            find: async (query, direction, options, incremental = false) => {
                const view = viewRef.current;
                if (!view) {
                    none();
                    return false;
                }
                return view.find(query, direction, options, incremental);
            },
            clearSearch: () => (viewRef.current ? viewRef.current.clearSearch() : none()),
            getSelection: () => viewRef.current?.getSelection() ?? "",
            copySelection: async () => viewRef.current?.copySelection() ?? false,
            pasteClipboard: async () => viewRef.current?.pasteClipboard() ?? false,
            selectAll: () => viewRef.current?.selectAll(),
            copyScrollback: async () => viewRef.current?.copyScrollback() ?? false,
            clear: () => viewRef.current?.clear(),
            focus: () => viewRef.current?.focus(),
        };
    }
    const bootingRef = useRef(false);
    const activeRef = useRef(active);
    activeRef.current = active;
    const visibleRef = useRef(visible);
    visibleRef.current = visible;
    const bootRef = useRef<() => void>(() => {});
    const resizeRef = useRef<() => void>(() => {});

    useEffect(() => {
        if (!shouldMount) return;
        const host = hostRef.current!;
        let disposed = false;
        let cleanup = () => {};
        let restartTimer: number | null = null;
        let stableTimer: number | null = null;
        const restartBackoff = new RendererRestartBackoff();
        let startNow = () => {};

        const scheduleRestart = () => {
            if (disposed || restartTimer !== null) return;
            const decision = restartBackoff.next(performance.now());
            host.dataset.terminalOutput = decision.throttled ? "cooldown" : "recovering";
            performanceTelemetry.incrementCounter(
                decision.throttled ? "terminal.renderer-restarts.throttled" : "terminal.renderer-restarts.scheduled",
            );
            restartTimer = window.setTimeout(() => {
                restartTimer = null;
                delete host.dataset.terminalOutput;
                // A hidden terminal whose start keeps failing would otherwise retry off screen; showing it boots it.
                if (visibleRef.current) startNow();
            }, decision.delayMs);
        };

        const boot = async () => {
            if (disposed || viewRef.current || bootingRef.current) return;
            bootingRef.current = true;
            const pty = ptyController.current;
            if (!pty) {
                bootingRef.current = false;
                return;
            }
            const pid = await pty.start().catch(() => null);
            if (disposed || pid === null) {
                bootingRef.current = false;
                if (!disposed && pid === null) scheduleRestart();
                return;
            }

            const disposers: Array<() => void> = [];
            const view = new TerminalView({
                host,
                pid,
                font: font(currentTerminalFontSize()),
                theme: currentTerminalTheme(),
                input: (input) => settle(pty.input(input)),
                onFindRequest: (seed) => callbacks.current.onFindRequest?.(seed),
                onSearchResults: (results) => callbacks.current.onSearchResults?.(results),
                onExit: () => {
                    if (!disposed) callbacks.current.onExit?.();
                },
            });
            disposers.push(() => view.dispose());
            const release = () => {
                for (let index = disposers.length - 1; index >= 0; index -= 1) {
                    try {
                        disposers[index]();
                    } catch {
                        performanceTelemetry.incrementCounter("terminal.renderer-cleanup-errors");
                    }
                }
                disposers.length = 0;
                if (viewRef.current === view) viewRef.current = null;
                resizeRef.current = () => {};
                bootingRef.current = false;
            };
            cleanup = release;
            view.setVisible(visibleRef.current);
            disposers.push(registerTerminal({ setTheme: (theme) => view.setTheme(theme) }, () => visibleRef.current));

            let size = view.fit();
            try {
                await pty.resize(size.cols, size.rows);
            } catch {
                /* The controller reports it; the next resize tries again. */
            }
            if (disposed) {
                release();
                return;
            }
            let attachment;
            try {
                attachment = await pty.attach((chunk) => view.receive(chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk)));
            } catch {
                performanceTelemetry.incrementCounter("terminal.renderer-attach-errors");
                release();
                scheduleRestart();
                return;
            }
            disposers.push(() => settle(attachment.detach()));
            callbacks.current.onShellMetadata?.(attachment.shell);
            if (disposed) {
                release();
                return;
            }
            view.start(attachment);

            let resizeFrame: number | null = null;
            let stopWaitingForStill: (() => void) | null = null;
            const sendSize = () => {
                stopWaitingForStill = null;
                settle(pty.resize(size.cols, size.rows));
            };
            const resizeNow = () => {
                resizeFrame = null;
                const next = view.fit();
                if (next.cols === size.cols && next.rows === size.rows) return;
                size = next;
                /* A program redraws its whole screen on every resize, so a rail or desk
                   sliding past tells it the size once, where the slide lands. */
                if (stageMoving()) stopWaitingForStill ??= whenStageStill(sendSize);
                else sendSize();
            };
            const resize = () => {
                if (resizeFrame === null) resizeFrame = window.requestAnimationFrame(resizeNow);
            };
            resizeRef.current = resize;
            disposers.push(() => {
                if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame);
                stopWaitingForStill?.();
            });
            disposers.push(
                registerTerminalFontSize({
                    setFontSize: (next) => view.setFont(font(next)),
                    refit: resize,
                }),
            );
            const observer = new ResizeObserver(resize);
            observer.observe(host);
            disposers.push(() => observer.disconnect());

            viewRef.current = view;
            bootingRef.current = false;
            stableTimer = window.setTimeout(() => {
                stableTimer = null;
                restartBackoff.reset();
                performanceTelemetry.incrementCounter("terminal.renderer-restarts.stable");
            }, RENDERER_STABLE_RESET_MS);
            if (activeRef.current) view.focus();
        };

        const fontsThenBoot = () =>
            void Promise.all([
                document.fonts.load(`${FONT_WEIGHT} ${FONT_SIZE}px "JetBrainsMono NF"`),
                document.fonts.load(`${FONT_WEIGHT_BOLD} ${FONT_SIZE}px "JetBrainsMono NF"`),
            ]).then(() => {
                void boot();
                window.setTimeout(() => {
                    void Promise.all([
                        document.fonts.load(`italic ${FONT_WEIGHT} ${FONT_SIZE}px "JetBrainsMono NF"`),
                        document.fonts.load(`italic ${FONT_WEIGHT_BOLD} ${FONT_SIZE}px "JetBrainsMono NF"`),
                    ]).catch(swallow("load italic fonts"));
                }, 0);
            }, boot);
        let stopWaitingToStart: (() => void) | null = null;
        const startWhenStill = () => {
            if (disposed || viewRef.current || bootingRef.current || stopWaitingToStart) return;
            stopWaitingToStart = startInTurn(() => {
                stopWaitingToStart = null;
                fontsThenBoot();
            }, activeRef.current);
        };
        bootRef.current = startWhenStill;
        startNow = fontsThenBoot;
        startWhenStill();

        return () => {
            disposed = true;
            stopWaitingToStart?.();
            bootRef.current = () => {};
            if (restartTimer !== null) window.clearTimeout(restartTimer);
            if (stableTimer !== null) window.clearTimeout(stableTimer);
            delete host.dataset.terminalOutput;
            cleanup();
        };
    }, [shouldMount, hostRef, ptyController]);

    // A screen sliding in keeps its size, so its terminal measures itself once the slide lands rather than in the middle of it.
    useEffect(() => {
        viewRef.current?.setVisible(visible);
        if (!visible) return;
        if (viewRef.current) return whenStageStill(() => resizeRef.current());
        if (shouldMount) bootRef.current();
    }, [visible, shouldMount]);

    useEffect(() => {
        if (!active) return;
        if (viewRef.current) {
            viewRef.current.focus();
            return whenStageStill(() => resizeRef.current());
        }
        if (shouldMount) bootRef.current();
    }, [active, shouldMount]);

    return controllerRef.current;
}
