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
}));

vi.mock("@xterm/xterm", () => ({
    Terminal: class {
        readonly options: Record<string, unknown> = {};
        readonly cols = 80;
        readonly rows = 24;
        readonly element = document.createElement("div");
        readonly buffer = { active: { viewportY: 0, baseY: 0, type: "normal" } };
        readonly modes = { mouseTrackingMode: "none", applicationCursorKeysMode: false };
        readonly dispose = vi.fn();

        constructor() {
            mocks.terminals.push(this);
        }

        loadAddon() {}
        open(host: HTMLElement) {
            host.append(this.element);
        }
        onTitleChange() {
            return { dispose: mocks.titleDispose };
        }
        attachCustomWheelEventHandler() {}
        attachCustomKeyEventHandler() {}
        refresh() {}
        write() {}
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
    mocks.unregisterTheme.mockClear();
    mocks.searchDispose.mockClear();
    mocks.titleDispose.mockClear();
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
        await act(async () => {
            await Promise.resolve();
            await Promise.resolve();
            await Promise.resolve();
        });

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
});
