import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DiagnosticsController } from "../workbench/diagnosticsController";
import type { ProjectDiagnosticsLease } from "../workbench/projectDiagnostics";
import { useLspBridge } from "./useLspBridge";

const mocks = vi.hoisted(() => ({
    start: vi.fn(),
    open: vi.fn(),
    change: vi.fn(),
    changeIncremental: vi.fn(),
    save: vi.fn(),
    close: vi.fn(),
    install: vi.fn(),
    acquire: vi.fn(),
    noteServerStarted: vi.fn(),
    release: vi.fn(),
    languageServersAllowed: vi.fn(),
}));

vi.mock("../languageServerTrust", () => ({ languageServersAllowed: mocks.languageServersAllowed }));

vi.mock("../api/lsp", async (importOriginal) => {
    const actual = await importOriginal<typeof import("../api/lsp")>();
    return {
        ...actual,
        lsp: {
            ...actual.lsp,
            start: mocks.start,
            open: mocks.open,
            change: mocks.change,
            changeIncremental: mocks.changeIncremental,
            save: mocks.save,
            close: mocks.close,
            install: mocks.install,
        },
    };
});

vi.mock("../workbench/projectDiagnostics", () => ({
    projectDiagnosticsRuntime: {
        acquire: mocks.acquire,
        noteServerStarted: mocks.noteServerStarted,
    },
}));

function diagnosticsLease(project: string, controller: DiagnosticsController): ProjectDiagnosticsLease {
    return {
        project,
        controller,
        ready: Promise.resolve(),
        released: false,
        getSnapshot: vi.fn(),
        subscribe: vi.fn(),
        release: mocks.release,
    } as unknown as ProjectDiagnosticsLease;
}

function deferred<T>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    const promise = new Promise<T>((resolvePromise) => {
        resolve = resolvePromise;
    });
    return { promise, resolve };
}

describe("useLspBridge diagnostics lifecycle", () => {
    beforeEach(() => {
        vi.clearAllMocks();
        mocks.languageServersAllowed.mockResolvedValue(true);
        mocks.start.mockResolvedValue(undefined);
        mocks.open.mockResolvedValue(undefined);
        mocks.change.mockResolvedValue(undefined);
        mocks.changeIncremental.mockResolvedValue(undefined);
        mocks.save.mockResolvedValue(undefined);
        mocks.close.mockResolvedValue(undefined);
        mocks.install.mockResolvedValue("");
        mocks.noteServerStarted.mockReturnValue(1);
        mocks.acquire.mockImplementation((project: string) => diagnosticsLease(project, { project } as unknown as DiagnosticsController));
    });

    afterEach(cleanup);

    it("acquires project ownership and activates diagnostics only after a successful server start", async () => {
        const { result, unmount } = renderHook(() => useLspBridge("/repo"));
        await waitFor(() => expect(result.current.diagnostics).toMatchObject({ project: "/repo" }));

        await act(() => result.current.openDoc("/repo/src/app.ts", "const app = true;"));

        expect(mocks.acquire).toHaveBeenCalledWith("/repo");
        expect(mocks.start).toHaveBeenCalledWith("/repo", "typescript");
        expect(mocks.noteServerStarted).toHaveBeenCalledWith("/repo", "typescript");
        expect(mocks.open).toHaveBeenCalledWith("/repo", "typescript", "/repo/src/app.ts", "const app = true;", "typescript");
        expect(mocks.start.mock.invocationCallOrder[0]).toBeLessThan(mocks.noteServerStarted.mock.invocationCallOrder[0]);
        expect(mocks.noteServerStarted.mock.invocationCallOrder[0]).toBeLessThan(mocks.open.mock.invocationCallOrder[0]);

        unmount();
        expect(mocks.release).toHaveBeenCalledOnce();
        await waitFor(() => expect(mocks.close).toHaveBeenCalledWith("/repo", "typescript", "/repo/src/app.ts"));
    });

    it("starts no language server for a project the user has not allowed", async () => {
        mocks.languageServersAllowed.mockResolvedValue(false);
        const { result } = renderHook(() => useLspBridge("/repo"));
        await waitFor(() => expect(result.current.diagnostics).not.toBeNull());

        await act(() => result.current.openDoc("/repo/src/app.ts", "export {};"));

        expect(mocks.languageServersAllowed).toHaveBeenCalledWith("/repo");
        expect(mocks.start).not.toHaveBeenCalled();
        expect(mocks.open).not.toHaveBeenCalled();
    });

    it("does not activate diagnostics when native server startup fails", async () => {
        mocks.start.mockRejectedValueOnce(new Error("language server unavailable"));
        const { result } = renderHook(() => useLspBridge("/repo"));
        await waitFor(() => expect(result.current.diagnostics).not.toBeNull());

        await act(() => result.current.openDoc("/repo/src/app.ts", "export {};"));

        expect(mocks.noteServerStarted).not.toHaveBeenCalled();
        expect(mocks.open).not.toHaveBeenCalled();
    });

    it("releases the old project and never exposes its controller after a cwd change", async () => {
        const props = { cwd: "/first" };
        const { result, rerender } = renderHook(() => useLspBridge(props.cwd));
        await waitFor(() => expect(result.current.diagnostics).toMatchObject({ project: "/first" }));

        props.cwd = "/second";
        rerender();
        await waitFor(() => expect(result.current.diagnostics).toMatchObject({ project: "/second" }));
        expect(mocks.release).toHaveBeenCalledOnce();
    });

    it("drops a server start that settles after the project lifecycle changes", async () => {
        const startup = deferred<void>();
        mocks.start.mockReturnValueOnce(startup.promise);
        const props = { cwd: "/first" };
        const { result, rerender } = renderHook(() => useLspBridge(props.cwd));
        await waitFor(() => expect(result.current.diagnostics).not.toBeNull());

        const opening = result.current.openDoc("/first/src/app.ts", "export {};");
        props.cwd = "/second";
        rerender();
        startup.resolve();
        await act(() => opening);

        expect(mocks.noteServerStarted).not.toHaveBeenCalled();
        expect(mocks.open).not.toHaveBeenCalled();
    });

    it("does not acquire a diagnostics runtime for a non-project editor", () => {
        const { result } = renderHook(() => useLspBridge(""));
        expect(result.current.diagnostics).toBeNull();
        expect(mocks.acquire).not.toHaveBeenCalled();
    });
});
