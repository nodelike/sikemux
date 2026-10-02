import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { GitFile } from "../api/git";
import { setState } from "../state/store";
import { DiffPane } from "./DiffPane";

const h = vi.hoisted(() => ({
    overview: { status: "ok", data: undefined as unknown, error: undefined as string | undefined, refresh: vi.fn() },
    enabled: vi.fn(),
    invalidated: vi.fn(),
    saved: null as null | (() => void),
}));

vi.mock("../state/resources", async (original) => ({
    ...(await original<typeof import("../state/resources")>()),
    useResourceEnabled: (enabled: boolean) => {
        h.enabled(enabled);
        return h.overview;
    },
}));
vi.mock("./DiffEditor", () => ({ invalidateDiffContentCache: h.invalidated }));
vi.mock("./CommitReview", () => ({
    CommitReview: ({ rev, subtitle }: { rev: string; subtitle: string }) => <div>{`commit ${rev} ${subtitle}`}</div>,
}));
vi.mock("./MergeReview", () => ({
    MergeReview: ({ files, focusPath, onSaved }: { files: GitFile[]; focusPath?: string; onSaved: () => void }) => {
        h.saved = onSaved;
        return <div>{`${files.length} files, focused on ${focusPath}`}</div>;
    },
}));

const withFiles = (paths: string[]) => ({ status: { files: paths.map((path) => ({ path, index: " ", worktree: "M" })) } });

beforeEach(() => {
    h.overview = { status: "ok", data: withFiles(["a.ts", "b.ts"]), error: undefined, refresh: vi.fn().mockResolvedValue(undefined) };
    h.enabled.mockClear();
    h.invalidated.mockClear();
    setState({ diffTarget: {} });
});
afterEach(cleanup);

it("asks for a project before there is one to review", () => {
    render(<DiffPane cwd="" active />);
    expect(screen.getByText("open a project to review changes")).toBeInTheDocument();
    expect(h.enabled).toHaveBeenLastCalledWith(false);
});

it("shows why the repository could not be read", () => {
    h.overview = { ...h.overview, status: "error", data: undefined, error: "not a git repository" };
    render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("not a git repository")).toBeInTheDocument();
});

it("reviews a commit when one was asked for, even while the changes load", () => {
    h.overview = { ...h.overview, status: "loading", data: undefined };
    setState({ diffTarget: { "/repo": { kind: "commit", rev: "abc123", subject: "fix things" } } });
    render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("commit abc123 fix things")).toBeInTheDocument();
});

it("says it is reading until the first changes arrive, then that there are none", () => {
    h.overview = { ...h.overview, status: "loading", data: undefined };
    const { rerender } = render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("reading the repository…")).toBeInTheDocument();

    h.overview = { ...h.overview, status: "ok", data: withFiles([]) };
    rerender(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("no changes to review")).toBeInTheDocument();
});

it("opens on the file that was asked for, or the first change when that file has none", () => {
    setState({ diffTarget: { "/repo": { kind: "worktree", path: "b.ts" } } });
    const { rerender } = render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("2 files, focused on b.ts")).toBeInTheDocument();

    act(() => setState({ diffTarget: { "/repo": { kind: "worktree", path: "gone.ts" } } }));
    rerender(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("2 files, focused on a.ts")).toBeInTheDocument();
});

it("drops cached file contents and rereads the repository after a save", () => {
    render(<DiffPane cwd="/repo" active />);
    act(() => h.saved?.());
    expect(h.invalidated).toHaveBeenCalledWith("/repo");
    expect(h.overview.refresh).toHaveBeenCalled();
});
