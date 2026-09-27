import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import type { NativePtyController } from "./usePty";
import { useXterm } from "./useXterm";

const mocks = vi.hoisted(() => ({
    terminals: [] as Array<{ dispose: ReturnType<typeof vi.fn> }>,
    unregisterTheme: vi.fn(),
    searchDispose: vi.fn(),
    titleDispose: vi.fn(),
    webglDispose: vi.fn(),
    terminalOptions: [] as Array<Record<string, unknown>>,
    invoke: vi.fn(async () => undefined),
}));

vi.mock("../api/invoke", () => ({ invokeCommand: mocks.invoke }));

vi.mock("@xterm/xterm", () => ({
    Terminal: class {
        readonly options: Record<string, unknown> = {};
        readonly cols = 80;
        readonly rows = 24;
        readonly element = document.createElement("div");
        readonly buffer = { active: { viewportY: 0, baseY: 0, type: "normal" } };
        readonly modes = { mouseTrackingMode: "none", applicationCursorKeysMode: false };
        readonly dispose = vi.fn();

        constructor(options: Record<string, unknown>) {
            mocks.terminals.push(this);
            mocks.terminalOptions.push(options);
        }

        write(_data: unknown, done?: () => void) {
            done?.();
        }

        loadAddon() {}
        open(host: HTMLElement) {
            host.append(this.element);
        }
        onTitleChange() {
            return { dispose: mocks.titleDispose };
        }
        onData() {
            return { dispose: vi.fn() };
        }
        attachCustomWheelEventHandler() {}
        attachCustomKeyEventHandler() {}
        refresh() {}
        scrollToBottom() {}
        focus() {}
        getSelection() {
            return "";
        }
        selectAll() {}
        clear() {}
        paste() {}
    },
}));

vi.mock("@xterm/addon-fit", () => ({
    FitAddon: class {
        fit() {}
    },
}));

vi.mock("@xterm/addon-search", () => ({
    SearchAddon: class {
        onDidChangeResults() {
            return { dispose: mocks.searchDispose };
        }
        clearDecorations() {}
        findNext() {
            return false;
        }
        findPrevious() {
            return false;
        }
    },
}));

vi.mock("@xterm/addon-serialize", () => ({
    SerializeAddon: class {},
}));

vi.mock("@xterm/addon-web-links", () => ({
    WebLinksAddon: class {},
}));

vi.mock("@xterm/addon-webgl", () => ({
    WebglAddon: class {
        onContextLoss() {
            return { dispose: vi.fn() };
        }
        dispose = mocks.webglDispose;
    },
}));

vi.mock("../themes/bus", () => ({
    currentTerminalTheme: () => ({ background: "rgba(0, 0, 0, 0)" }),
    registerTerminal: () => mocks.unregisterTheme,
}));

function Harness({ controller, onExit }: { controller: NativePtyController; onExit: () => void }) {
    const hostRef = useRef<HTMLDivElement>(null);
    const controllerRef = useRef(controller);
    useXterm({ hostRef, ptyController: controllerRef, shouldMount: true, active: true, visible: true, onExit });
    return <div ref={hostRef} />;
}

beforeEach(() => {
    vi.useFakeTimers();
    mocks.terminals.length = 0;
    mocks.terminalOptions.length = 0;
    mocks.invoke.mockClear();
    mocks.unregisterTheme.mockClear();
    mocks.searchDispose.mockClear();
    mocks.titleDispose.mockClear();
    mocks.webglDispose.mockClear();
    Object.defineProperty(document, "fonts", {
        configurable: true,
        value: { load: vi.fn().mockResolvedValue([]) },
    });
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("useXterm renderer boot", () => {
    it("releases provisional resources and retries without reporting process exit", async () => {
        const resize = vi.fn().mockRejectedValue(new Error("bridge unavailable"));
        const attach = vi.fn();
        const onExit = vi.fn();
        const controller = {
            start: vi.fn().mockResolvedValue(7),
            resize,
            attach,
            write: vi.fn().mockResolvedValue(undefined),
        } as unknown as NativePtyController;

        const view = render(<Harness controller={controller} onExit={onExit} />);
        await act(async () => vi.advanceTimersByTimeAsync(0));

        expect(resize).toHaveBeenCalledWith(80, 24);
        expect(attach).not.toHaveBeenCalled();
        expect(mocks.terminals).toHaveLength(1);
        expect(mocks.terminals[0].dispose).toHaveBeenCalledOnce();
        expect(mocks.unregisterTheme).toHaveBeenCalledOnce();
        expect(mocks.searchDispose).toHaveBeenCalledOnce();
        expect(mocks.titleDispose).toHaveBeenCalledOnce();
        expect(onExit).not.toHaveBeenCalled();
        expect((view.container.firstElementChild as HTMLElement).dataset.terminalOutput).toBe("recovering");

        await act(async () => vi.advanceTimersByTimeAsync(100));
        expect(mocks.terminals).toHaveLength(2);
    });

    it("renders through WebGL without an explicit environment opt-in", async () => {
        const controller = {
            start: vi.fn().mockResolvedValue(7),
            resize: vi.fn().mockResolvedValue(undefined),
            attach: vi.fn().mockResolvedValue({
                snapshot: new Uint8Array(),
                alternateScreen: false,
                shell: null,
                activate: vi.fn(),
                detach: vi.fn().mockResolvedValue(undefined),
            }),
            write: vi.fn().mockResolvedValue(undefined),
        } as unknown as NativePtyController;

        const view = render(<Harness controller={controller} onExit={vi.fn()} />);
        await act(async () => vi.advanceTimersByTimeAsync(0));

        expect((view.container.firstElementChild as HTMLElement).dataset.terminalRenderer).toBe("webgl");
    });

    it("acks the bytes the native channel delivered, not the replayed snapshot", async () => {
        const ack = vi.fn();
        let deliver: (chunk: Uint8Array) => void = () => {};
        const controller = {
            start: vi.fn().mockResolvedValue(7),
            resize: vi.fn().mockResolvedValue(undefined),
            attach: vi.fn().mockImplementation((listener: (chunk: Uint8Array) => void) => {
                deliver = listener;
                return Promise.resolve({
                    snapshot: new Uint8Array([1, 2, 3, 4]),
                    alternateScreen: false,
                    shell: null,
                    activate: vi.fn(),
                    ack,
                    detach: vi.fn().mockResolvedValue(undefined),
                });
            }),
            write: vi.fn().mockResolvedValue(undefined),
        } as unknown as NativePtyController;

        render(<Harness controller={controller} onExit={vi.fn()} />);
        await act(async () => vi.advanceTimersByTimeAsync(50));

        // The snapshot came back from the attach call, so it owes nothing.
        expect(ack).toHaveBeenCalledWith(0);

        ack.mockClear();
        act(() => deliver(new Uint8Array(64)));
        await act(async () => vi.advanceTimersByTimeAsync(50));
        expect(ack).toHaveBeenCalledWith(64);
    });

    it("opens a program's web hyperlinks in the browser and ignores other schemes", async () => {
        const controller = {
            start: vi.fn().mockResolvedValue(7),
            resize: vi.fn().mockResolvedValue(undefined),
            attach: vi.fn().mockResolvedValue({
                snapshot: new Uint8Array(),
                alternateScreen: false,
                shell: null,
                activate: vi.fn(),
                detach: vi.fn().mockResolvedValue(undefined),
            }),
            write: vi.fn().mockResolvedValue(undefined),
        } as unknown as NativePtyController;

        render(<Harness controller={controller} onExit={vi.fn()} />);
        await act(async () => vi.advanceTimersByTimeAsync(0));

        const linkHandler = mocks.terminalOptions[0].linkHandler as { activate: (event: MouseEvent, text: string) => void };
        const click = () => new MouseEvent("click", { cancelable: true });
        const web = click();
        linkHandler.activate(web, "https://example.com/a");
        expect(web.defaultPrevented).toBe(true);
        expect(mocks.invoke).toHaveBeenCalledWith("open_url", { url: "https://example.com/a", app: null, shortcut: null });

        mocks.invoke.mockClear();
        const other = click();
        linkHandler.activate(other, "file:///etc/passwd");
        expect(other.defaultPrevented).toBe(true);
        expect(mocks.invoke).not.toHaveBeenCalledWith("open_url", expect.anything());
    });
});
