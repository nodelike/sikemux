import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { GitFile } from "../api/git";
import { setState } from "../state/store";
import { DiffPane } from "./DiffPane";

const h = vi.hoisted(() => ({
    overview: { status: "ok", data: undefined as unknown, error: undefined as string | undefined, refresh: vi.fn() },
}));

vi.mock("../state/resources", async (original) => ({
    ...(await original<typeof import("../state/resources")>()),
    useResourceEnabled: () => h.overview,
}));
vi.mock("./DiffEditor", () => ({ invalidateDiffContentCache: vi.fn() }));
vi.mock("./CommitReview", () => ({
    CommitReview: ({ rev, subtitle }: { rev: string; subtitle: string }) => <div>{`commit ${rev} ${subtitle}`}</div>,
}));
vi.mock("./MergeReview", () => ({
    MergeReview: ({ files, focusPath }: { files: GitFile[]; focusPath?: string }) => <div>{`${files.length} files, focused on ${focusPath}`}</div>,
}));

const withFiles = (paths: string[]) => ({ status: { files: paths.map((path) => ({ path, index: " ", worktree: "M" })) } });

beforeEach(() => {
    h.overview = { status: "ok", data: withFiles(["a.ts", "b.ts"]), error: undefined, refresh: vi.fn().mockResolvedValue(undefined) };
    setState({ diffTarget: {} });
});
afterEach(cleanup);

it("reviews a commit when one was asked for, even while the changes load", () => {
    h.overview = { ...h.overview, status: "loading", data: undefined };
    setState({ diffTarget: { "/repo": { kind: "commit", rev: "abc123", subject: "fix things" } } });
    render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("commit abc123 fix things")).toBeInTheDocument();
});

it("opens on the file that was asked for, or the first change when that file has none", () => {
    setState({ diffTarget: { "/repo": { kind: "worktree", path: "b.ts" } } });
    const { rerender } = render(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("2 files, focused on b.ts")).toBeInTheDocument();

    act(() => setState({ diffTarget: { "/repo": { kind: "worktree", path: "gone.ts" } } }));
    rerender(<DiffPane cwd="/repo" active />);
    expect(screen.getByText("2 files, focused on a.ts")).toBeInTheDocument();
});
