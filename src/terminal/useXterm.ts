import { useEffect, useRef, type RefObject } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon, type ISearchOptions, type ISearchResultChangeEvent } from "@xterm/addon-search";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebLinksAddon } from "@xterm/addon-web-links";
import type { WebglAddon } from "@xterm/addon-webgl";
import { invokeCommand as invoke } from "../api/invoke";
import { currentTerminalTheme, registerTerminal } from "../themes/bus";
import { IS_MACOS } from "../lib/platform";
import {
    completeInitialReplay,
    initialReplayScrollState,
    noteUserViewportGesture,
    replaySerializedNormalBuffer,
    serializeNormalBuffer,
    type SerializedNormalBuffer,
} from "./sessionState";
import { alternateScreenWheelFallbackSequence } from "./wheelNavigation";
import { needsTerminalRedraw } from "./redraw";
import { terminalWebglRequested, type TerminalRenderer } from "./renderer";
import { isTerminalFindShortcut, safeWebUrl, sanitizeTerminalTitle, terminalBufferText, type TerminalSearchOptions } from "./interactions";
import { scheduleNextFrame } from "../lib/instrumentation";
import { performanceTelemetry } from "../lib/performance";
import type { PtyAttachment, PtyOutputChunk, PtyShellMetadataSnapshot } from "./ptyController";
import { RendererRestartBackoff } from "./restartBackoff";
import type { NativePtyController } from "./usePty";

const FONT = '"JetBrainsMono NF", "JetBrainsMono Nerd Font", monospace';
const FONT_WEIGHT = 500;
const FONT_WEIGHT_BOLD = 700;
const SCROLLBACK = 10_000;
const MAX_RENDERER_BACKLOG_BYTES = 8 * 1024 * 1024;
const MAX_RENDERER_BACKLOG_CHUNKS = 512;
const RENDERER_STABLE_RESET_MS = 5_000;
const WEBGL_REQUESTED = terminalWebglRequested(import.meta.env.VITE_TERMINAL_WEBGL);

function settlePtyOperation(operation: Promise<unknown>): void {
    // PtyLifecycleController records the operation failure. Renderer event
    // handlers only need to contain the rejection so it cannot reach window.
    void operation.catch(() => {});
}

const META_CHORDS: Record<string, string> = {
    "Meta+ArrowLeft": "\x01", // Ctrl-A: start of line
    "Meta+ArrowRight": "\x05", // Ctrl-E: end of line
    "Meta+Backspace": "\x15", // Ctrl-U: kill to start of line
};
const ALT_CHORDS: Record<string, string> = {
    "Alt+ArrowLeft": "\x1bb", // Esc-b: back one word
    "Alt+ArrowRight": "\x1bf", // Esc-f: forward one word
    "Alt+Backspace": "\x1b\x7f", // Esc-DEL: delete previous word
};

export interface TerminalController {
    find(query: string, direction: "next" | "previous", options: TerminalSearchOptions, incremental?: boolean): boolean;
    clearSearch(): void;
    getSelection(): string;
    copySelection(): Promise<boolean>;
    pasteClipboard(): Promise<boolean>;
    selectAll(): void;
    copyScrollback(): Promise<boolean>;
    clear(): void;
    focus(): void;
}

interface TerminalTarget {
    term: Terminal;
    search: SearchAddon;
}

const SEARCH_DECORATIONS: NonNullable<ISearchOptions["decorations"]> = {
    matchBackground: "#433866",
    matchOverviewRuler: "#7660a8",
    activeMatchBackground: "#a277ff",
    activeMatchColorOverviewRuler: "#a277ff",
};

export function useXterm(opts: {
    hostRef: RefObject<HTMLDivElement | null>;
    ptyController: RefObject<NativePtyController | null>;
    shouldMount: boolean;
    active: boolean;
    visible: boolean;
    onFindRequest?: (seed: string) => void;
    onSearchResults?: (result: ISearchResultChangeEvent) => void;
    onTitleChange?: (title: string) => void;
    onShellMetadata?: (metadata: PtyShellMetadataSnapshot | null) => void;
    onExit?: () => void;
}): TerminalController {
    const { hostRef, ptyController, shouldMount, active, visible } = opts;
    const onFindRequestRef = useRef(opts.onFindRequest);
    onFindRequestRef.current = opts.onFindRequest;
    const onSearchResultsRef = useRef(opts.onSearchResults);
    onSearchResultsRef.current = opts.onSearchResults;
    const onTitleChangeRef = useRef(opts.onTitleChange);
    onTitleChangeRef.current = opts.onTitleChange;
    const onShellMetadataRef = useRef(opts.onShellMetadata);
    onShellMetadataRef.current = opts.onShellMetadata;
    const onExitRef = useRef(opts.onExit);
    onExitRef.current = opts.onExit;
    const termRef = useRef<Terminal | null>(null);
    const targetRef = useRef<TerminalTarget | null>(null);
    const controllerRef = useRef<TerminalController | null>(null);
    if (!controllerRef.current) {
        controllerRef.current = {
            find: (query, direction, options, incremental = false) => {
                const target = targetRef.current;
                if (!target || !query) {
                    target?.search.clearDecorations();
                    onSearchResultsRef.current?.({ resultIndex: -1, resultCount: 0 });
                    return false;
                }
                const searchOptions: ISearchOptions = { ...options, incremental, decorations: SEARCH_DECORATIONS };
                try {
                    return direction === "next" ? target.search.findNext(query, searchOptions) : target.search.findPrevious(query, searchOptions);
                } catch {
                    onSearchResultsRef.current?.({ resultIndex: -1, resultCount: 0 });
                    return false;
                }
            },
            clearSearch: () => {
                targetRef.current?.search.clearDecorations();
                onSearchResultsRef.current?.({ resultIndex: -1, resultCount: 0 });
            },
            getSelection: () => targetRef.current?.term.getSelection() ?? "",
            copySelection: async () => {
                const selection = targetRef.current?.term.getSelection() ?? "";
                if (!selection) return false;
                await navigator.clipboard.writeText(selection);
                return true;
            },
            pasteClipboard: async () => {
                const target = targetRef.current;
                if (!target) return false;
                const text = await navigator.clipboard.readText();
                if (!text) return false;
                target.term.paste(text);
                return true;
            },
            selectAll: () => targetRef.current?.term.selectAll(),
            copyScrollback: async () => {
                const target = targetRef.current;
                if (!target) return false;
                const text = terminalBufferText(target.term.buffer.active);
                if (!text) return false;
                await navigator.clipboard.writeText(text);
                return true;
            },
            clear: () => {
                targetRef.current?.search.clearDecorations();
                targetRef.current?.term.clear();
                onSearchResultsRef.current?.({ resultIndex: -1, resultCount: 0 });
            },
            focus: () => targetRef.current?.term.focus(),
        };
    }
    const bootingRef = useRef(false);
    const activeRef = useRef(active);
    activeRef.current = active;
    const visibleRef = useRef(visible);
    visibleRef.current = visible;
    const bootRef = useRef<() => void>(() => {});
    const resizeRef = useRef<() => void>(() => {});
    const serializedNormalRef = useRef<SerializedNormalBuffer | null>(null);

    useEffect(() => {
        if (!shouldMount) return;
        const host = hostRef.current!;
        let disposed = false;
        let cleanup = () => {};
        let restartTimer: number | null = null;
        let stableTimer: number | null = null;
        const restartBackoff = new RendererRestartBackoff();

        const scheduleRendererRestart = () => {
            if (disposed || restartTimer !== null) return;
            const decision = restartBackoff.next(performance.now());
            host.dataset.terminalOutput = decision.throttled ? "cooldown" : "recovering";
            performanceTelemetry.incrementCounter(
                decision.throttled ? "terminal.renderer-restarts.throttled" : "terminal.renderer-restarts.scheduled",
            );
            restartTimer = window.setTimeout(() => {
                restartTimer = null;
                delete host.dataset.terminalOutput;
                bootRef.current();
            }, decision.delayMs);
        };

        const markRendererStableLater = () => {
            if (stableTimer !== null) window.clearTimeout(stableTimer);
            stableTimer = window.setTimeout(() => {
                stableTimer = null;
                restartBackoff.reset();
                delete host.dataset.terminalOutput;
                performanceTelemetry.incrementCounter("terminal.renderer-restarts.stable");
            }, RENDERER_STABLE_RESET_MS);
        };

        const boot = async () => {
            if (disposed || termRef.current || bootingRef.current) return;
            if (restartTimer !== null) {
                window.clearTimeout(restartTimer);
                restartTimer = null;
                delete host.dataset.terminalOutput;
            }
            bootingRef.current = true;
            const pty = ptyController.current;
            if (!pty) {
                bootingRef.current = false;
                return;
            }
            const pid = await pty.start().catch(() => null);
            if (disposed || pid === null) {
                bootingRef.current = false;
                if (!disposed && pid === null) scheduleRendererRestart();
                return;
            }

            let term: Terminal;
            try {
                term = new Terminal({
                    fontFamily: FONT,
                    fontSize: 13,
                    fontWeight: FONT_WEIGHT,
                    fontWeightBold: FONT_WEIGHT_BOLD,
                    lineHeight: 1.0,
                    theme: currentTerminalTheme(),
                    cursorBlink: true,
                    allowProposedApi: true,
                    allowTransparency: true,
                    macOptionIsMeta: IS_MACOS,
                    scrollback: SCROLLBACK,
                    scrollOnUserInput: true,
                    smoothScrollDuration: 0,
                });
            } catch {
                bootingRef.current = false;
                performanceTelemetry.incrementCounter("terminal.renderer-boot-errors");
                scheduleRendererRestart();
                return;
            }

            const resourceDisposers: Array<() => void> = [];
            let resourcesDisposed = false;
            const disposeResources = () => {
                if (resourcesDisposed) return;
                resourcesDisposed = true;
                resizeRef.current = () => {};
                if (stableTimer !== null) {
                    window.clearTimeout(stableTimer);
                    stableTimer = null;
                }
                for (let index = resourceDisposers.length - 1; index >= 0; index -= 1) {
                    try {
                        resourceDisposers[index]();
                    } catch {
                        performanceTelemetry.incrementCounter("terminal.renderer-cleanup-errors");
                    }
                }
                if (termRef.current === term) termRef.current = null;
                if (targetRef.current?.term === term) targetRef.current = null;
                delete host.dataset.terminalRenderer;
                bootingRef.current = false;
            };
            resourceDisposers.push(() => term.dispose());
            cleanup = disposeResources;

            try {
                const unregisterTheme = registerTerminal(term);
                resourceDisposers.push(unregisterTheme);
                const fit = new FitAddon();
                const search = new SearchAddon();
                const serializer = new SerializeAddon();
                const webLinks = new WebLinksAddon((event, uri) => {
                    event.preventDefault();
                    const url = safeWebUrl(uri);
                    if (!url) return;
                    void invoke("open_url", { url, app: null, shortcut: null }).catch((error) => console.warn("open terminal link failed", error));
                });
                term.loadAddon(fit);
                term.loadAddon(search);
                term.loadAddon(serializer);
                term.loadAddon(webLinks);
                term.open(host);
                targetRef.current = { term, search };
                const searchResultsSub = search.onDidChangeResults((result) => onSearchResultsRef.current?.(result));
                resourceDisposers.push(() => searchResultsSub.dispose());
                let lastTitle: string | null = null;
                const titleSub = term.onTitleChange((raw) => {
                    const title = sanitizeTerminalTitle(raw);
                    if (!title || title === lastTitle) return;
                    lastTitle = title;
                    onTitleChangeRef.current?.(title);
                });
                resourceDisposers.push(() => titleSub.dispose());
                let renderer: TerminalRenderer = "dom";
                let webgl: WebglAddon | null = null;
                let contextLossSub: { dispose(): void } | null = null;
                resourceDisposers.push(() => contextLossSub?.dispose());
                const setRenderer = (next: TerminalRenderer) => {
                    renderer = next;
                    host.dataset.terminalRenderer = next;
                };
                setRenderer("dom");
                if (WEBGL_REQUESTED) {
                    try {
                        const { WebglAddon } = await import("@xterm/addon-webgl");
                        if (disposed) {
                            cleanup();
                            return;
                        }
                        const addon = new WebglAddon();
                        webgl = addon;
                        contextLossSub = addon.onContextLoss(() => {
                            if (renderer !== "webgl") return;
                            console.warn("terminal WebGL context lost; falling back to DOM renderer");
                            setRenderer("dom");
                            addon.dispose();
                            webgl = null;
                            term.refresh(0, term.rows - 1);
                        });
                        term.loadAddon(addon);
                        setRenderer("webgl");
                    } catch (error) {
                        contextLossSub?.dispose();
                        contextLossSub = null;
                        webgl?.dispose();
                        webgl = null;
                        setRenderer("dom");
                        console.warn("terminal WebGL initialization failed; using DOM renderer", error);
                    }
                }
                if (disposed) {
                    cleanup();
                    return;
                }
                fit.fit();

                await pty.resize(term.cols, term.rows);
                if (disposed) {
                    cleanup();
                    return;
                }

                let outputFrame: number | null = null;
                let outputBusy = false;
                let closing = false;
                let finalized = false;
                let replayScrollState = initialReplayScrollState();
                let initialReplayOutputPending = false;
                const outputPending: Uint8Array[] = [];
                let outputPendingBytes = 0;
                let outputQueuedAt: number | null = null;
                let resyncing = false;
                let attached: PtyAttachment | null = null;
                const encoder = new TextEncoder();
                const isAtBottom = () => {
                    const buf = term.buffer.active;
                    return buf.viewportY >= buf.baseY;
                };
                const onUserViewportGesture = () => {
                    replayScrollState = noteUserViewportGesture(replayScrollState);
                };
                term.attachCustomWheelEventHandler(() => {
                    onUserViewportGesture();
                    return true;
                });
                const onWheel = (event: WheelEvent) => {
                    const sequence = alternateScreenWheelFallbackSequence({
                        defaultPrevented: event.defaultPrevented,
                        bufferType: term.buffer.active.type,
                        mouseTrackingMode: term.modes.mouseTrackingMode,
                        applicationCursorKeysMode: term.modes.applicationCursorKeysMode,
                        deltaX: event.deltaX,
                        deltaY: event.deltaY,
                    });
                    if (sequence === null) return;

                    // Alternate-screen TUIs have no xterm scrollback. xterm turns
                    // wheel gestures into cursor keys, but tiny trackpad deltas can
                    // round down to zero; guarantee movement in that fallback case.
                    event.preventDefault();
                    settlePtyOperation(pty.write(sequence));
                };
                host.addEventListener("wheel", onWheel, { passive: false });
                resourceDisposers.push(() => host.removeEventListener("wheel", onWheel));
                const viewport = host.querySelector<HTMLElement>(".xterm-viewport");
                const terminalElement = term.element;
                const onViewportKeyDown = (event: KeyboardEvent) => {
                    if (event.key === "PageUp" || event.key === "PageDown" || event.key === "Home" || event.key === "End") {
                        onUserViewportGesture();
                    }
                };
                viewport?.addEventListener("pointerdown", onUserViewportGesture);
                terminalElement?.addEventListener("touchstart", onUserViewportGesture, { capture: true, passive: true });
                terminalElement?.addEventListener("keydown", onViewportKeyDown, { capture: true });
                resourceDisposers.push(() => {
                    viewport?.removeEventListener("pointerdown", onUserViewportGesture);
                    terminalElement?.removeEventListener("touchstart", onUserViewportGesture, { capture: true });
                    terminalElement?.removeEventListener("keydown", onViewportKeyDown, { capture: true });
                });
                const finalizeCleanup = () => {
                    if (finalized || outputBusy || outputPending.length > 0) return;
                    finalized = true;
                    if (resyncing) {
                        serializedNormalRef.current = null;
                    } else {
                        try {
                            serializedNormalRef.current = serializeNormalBuffer(serializer, pid, SCROLLBACK);
                        } catch (error) {
                            serializedNormalRef.current = null;
                            console.warn("terminal normal-buffer serialization failed", error);
                        }
                    }
                    disposeResources();
                    if (resyncing && !disposed) {
                        scheduleRendererRestart();
                    }
                };
                const flushOutput = () => {
                    outputFrame = null;
                    if ((disposed && !closing) || outputBusy || outputPending.length === 0) return;
                    const completesInitialReplay = initialReplayOutputPending;
                    initialReplayOutputPending = false;
                    const flushStarted = performance.now();
                    if (outputQueuedAt !== null) performanceTelemetry.recordLatency("terminal.output.queue", flushStarted - outputQueuedAt);
                    outputQueuedAt = null;
                    let total = 0;
                    for (const chunk of outputPending) total += chunk.length;
                    const chunkCount = outputPending.length;
                    const merged = new Uint8Array(total);
                    let offset = 0;
                    for (const chunk of outputPending.splice(0)) {
                        merged.set(chunk, offset);
                        offset += chunk.length;
                    }
                    outputPendingBytes = 0;
                    performanceTelemetry.setGauge("terminal.last-queue-bytes", 0);
                    performanceTelemetry.recordLatency("terminal.output.merge", performance.now() - flushStarted);
                    performanceTelemetry.incrementCounter("terminal.output.flushes");
                    performanceTelemetry.incrementCounter("terminal.output.chunks", chunkCount);
                    performanceTelemetry.incrementCounter("terminal.output.bytes", total);
                    outputBusy = true;
                    const writeStarted = performance.now();
                    term.write(merged, () => {
                        outputBusy = false;
                        performanceTelemetry.recordLatency("terminal.xterm.write", performance.now() - writeStarted);
                        const frameStarted = performance.now();
                        scheduleNextFrame(() => {
                            performanceTelemetry.recordLatency("terminal.output.next-frame-proxy", performance.now() - frameStarted);
                        });
                        if (needsTerminalRedraw(merged)) {
                            // zsh-autosuggestions erases then redraws the input line.
                            // Force a complete canvas pass so transparent WKWebView
                            // terminals cannot retain the previous suggestion glyphs.
                            term.refresh(0, term.rows - 1);
                            performanceTelemetry.incrementCounter("terminal.full-redraws");
                        }
                        if (completesInitialReplay) {
                            const completion = completeInitialReplay(replayScrollState);
                            replayScrollState = completion.state;
                            if (completion.shouldScrollToBottom && !disposed) term.scrollToBottom();
                        }
                        if (outputPending.length > 0) {
                            if (closing) flushOutput();
                            else scheduleOutput();
                        } else if (closing) {
                            finalizeCleanup();
                        }
                    });
                };
                const scheduleOutput = () => {
                    if (!outputBusy && outputFrame == null) outputFrame = window.requestAnimationFrame(flushOutput);
                };
                const requestRendererResync = () => {
                    if (resyncing || closing || disposed) return;
                    resyncing = true;
                    outputPending.length = 0;
                    outputPendingBytes = 0;
                    outputQueuedAt = null;
                    performanceTelemetry.setGauge("terminal.last-queue-bytes", 0);
                    performanceTelemetry.incrementCounter("terminal.renderer-resyncs");
                    if (stableTimer !== null) {
                        window.clearTimeout(stableTimer);
                        stableTimer = null;
                    }
                    // Snapshot activation remains synchronous, then the renderer
                    // attachment is released before another animation frame queues.
                    queueMicrotask(() => cleanup());
                };
                const writeBytes = (bytes: Uint8Array) => {
                    if (bytes.length === 0 || resyncing) return;
                    if (outputPending.length >= MAX_RENDERER_BACKLOG_CHUNKS || outputPendingBytes + bytes.length > MAX_RENDERER_BACKLOG_BYTES) {
                        requestRendererResync();
                        return;
                    }
                    if (outputPending.length === 0) outputQueuedAt = performance.now();
                    outputPending.push(bytes);
                    outputPendingBytes += bytes.length;
                    performanceTelemetry.setGauge("terminal.last-queue-bytes", outputPendingBytes);
                    scheduleOutput();
                };
                const writeChunk = (chunk: PtyOutputChunk) => {
                    if (chunk.length === 0) {
                        writeBytes(encoder.encode("\r\n\x1b[38;5;245m[process exited]\x1b[0m\r\n"));
                        // Owners that must always keep a live shell (the Git pane)
                        // replace this pane on exit; everyone else ignores it.
                        if (!disposed && !closing) onExitRef.current?.();
                        return;
                    }
                    writeBytes(chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk));
                };

                try {
                    attached = await pty.attach(writeChunk);
                } catch {
                    // Attach errors are renderer-local unless the lifecycle
                    // controller explicitly reports an exited process. Preserve the
                    // durable owner and reattach after bounded backoff.
                    performanceTelemetry.incrementCounter("terminal.renderer-attach-errors");
                    cleanup();
                    scheduleRendererRestart();
                    return;
                }
                const detachAttachment = () => {
                    const current = attached;
                    attached = null;
                    if (current) settlePtyOperation(current.detach());
                };
                resourceDisposers.push(detachAttachment);
                const { snapshot, alternateScreen } = attached;
                onShellMetadataRef.current?.(attached.shell);
                if (disposed) {
                    cleanup();
                    return;
                }
                const serializedNormal = replaySerializedNormalBuffer(serializedNormalRef.current, pid, alternateScreen);
                if (serializedNormal !== null) writeBytes(encoder.encode(serializedNormal));
                if (snapshot.length > 0) writeChunk(snapshot);
                attached.activate();
                initialReplayOutputPending = outputPending.length > 0;
                if (!initialReplayOutputPending) replayScrollState = completeInitialReplay(replayScrollState).state;

                let pendingInput = "";
                let scheduled = false;
                const flushInput = () => {
                    scheduled = false;
                    if (!pendingInput) return;
                    const data = pendingInput;
                    pendingInput = "";
                    settlePtyOperation(pty.write(data));
                };
                const dataSub = term.onData((data) => {
                    pendingInput += data;
                    if (!scheduled) {
                        scheduled = true;
                        queueMicrotask(flushInput);
                    }
                });
                resourceDisposers.push(() => dataSub.dispose());

                term.attachCustomKeyEventHandler((e) => {
                    if (e.type !== "keydown") return true;
                    if (isTerminalFindShortcut(e, IS_MACOS)) {
                        onFindRequestRef.current?.(term.getSelection());
                        e.preventDefault();
                        e.stopPropagation();
                        return false;
                    }
                    if (e.code === "KeyR" && e.metaKey && e.altKey && !e.ctrlKey && !e.shiftKey) {
                        settlePtyOperation(invoke("pty_reset_modes", { id: pid }));
                        e.preventDefault();
                        e.stopPropagation();
                        return false;
                    }
                    if (e.key === "Enter" && e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) {
                        settlePtyOperation(pty.write("\x1b[13;2u"));
                        e.preventDefault();
                        e.stopPropagation();
                        return false;
                    }
                    const keyParts: string[] = [];
                    if (e.metaKey) keyParts.push("Meta");
                    if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "Backspace")) {
                        keyParts.push("Alt");
                    }
                    if (keyParts.length === 0) return true;
                    const sig = `${keyParts.join("+")}+${e.key}`;
                    const seq = META_CHORDS[sig] ?? ALT_CHORDS[sig];
                    if (seq === undefined) return true;
                    settlePtyOperation(pty.write(seq));
                    e.preventDefault();
                    e.stopPropagation();
                    return false;
                });

                let resizeFrame: number | null = null;
                let lastCols = term.cols;
                let lastRows = term.rows;
                const resizeNow = () => {
                    resizeFrame = null;
                    if (host.clientWidth === 0 || host.clientHeight === 0) return;
                    const stickToBottom = isAtBottom();
                    fit.fit();
                    if (stickToBottom) term.scrollToBottom();
                    if (term.cols === lastCols && term.rows === lastRows) return;
                    lastCols = term.cols;
                    lastRows = term.rows;
                    settlePtyOperation(pty.resize(term.cols, term.rows));
                };
                const resize = () => {
                    if (resizeFrame == null) resizeFrame = window.requestAnimationFrame(resizeNow);
                };
                resizeRef.current = resize;
                const ro = new ResizeObserver(resize);
                ro.observe(host);
                resourceDisposers.push(() => ro.disconnect());

                termRef.current = term;
                bootingRef.current = false;
                markRendererStableLater();
                if (visibleRef.current) window.requestAnimationFrame(resize);
                if (activeRef.current) term.focus();

                cleanup = () => {
                    if (closing) return;
                    closing = true;
                    resizeRef.current = () => {};
                    if (stableTimer !== null) {
                        window.clearTimeout(stableTimer);
                        stableTimer = null;
                    }
                    if (outputFrame != null) window.cancelAnimationFrame(outputFrame);
                    outputFrame = null;
                    if (resizeFrame != null) window.cancelAnimationFrame(resizeFrame);
                    outputPendingBytes = 0;
                    outputQueuedAt = null;
                    performanceTelemetry.setGauge("terminal.last-queue-bytes", 0);
                    detachAttachment();
                    termRef.current = null;
                    if (targetRef.current?.term === term) targetRef.current = null;
                    if (outputBusy) return;
                    if (outputPending.length > 0) flushOutput();
                    else finalizeCleanup();
                };
            } catch {
                performanceTelemetry.incrementCounter("terminal.renderer-boot-errors");
                cleanup();
                scheduleRendererRestart();
            }
        };

        const fontsThenBoot = () =>
            void Promise.all([
                document.fonts.load(`${FONT_WEIGHT} 13px "JetBrainsMono NF"`),
                document.fonts.load(`${FONT_WEIGHT_BOLD} 13px "JetBrainsMono NF"`),
            ]).then(() => {
                void boot();
                window.setTimeout(() => {
                    void Promise.all([
                        document.fonts.load(`italic ${FONT_WEIGHT} 13px "JetBrainsMono NF"`),
                        document.fonts.load(`italic ${FONT_WEIGHT_BOLD} 13px "JetBrainsMono NF"`),
                    ]).catch(() => {});
                }, 0);
            }, boot);
        bootRef.current = fontsThenBoot;

        fontsThenBoot();

        return () => {
            disposed = true;
            bootRef.current = () => {};
            if (restartTimer !== null) window.clearTimeout(restartTimer);
            if (stableTimer !== null) window.clearTimeout(stableTimer);
            restartTimer = null;
            stableTimer = null;
            delete host.dataset.terminalOutput;
            cleanup();
        };
    }, [shouldMount, hostRef, ptyController]);

    useEffect(() => {
        if (!visible) return;
        if (termRef.current) {
            window.requestAnimationFrame(resizeRef.current);
        } else if (shouldMount) {
            bootRef.current();
        }
    }, [visible, shouldMount]);

    useEffect(() => {
        if (!active) return;
        if (termRef.current) {
            window.requestAnimationFrame(resizeRef.current);
            termRef.current.focus();
        } else if (shouldMount) {
            bootRef.current();
        }
    }, [active, shouldMount]);

    return controllerRef.current!;
}
