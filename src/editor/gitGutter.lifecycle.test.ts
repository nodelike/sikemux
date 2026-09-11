import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, expect, it, vi } from "vitest";
import { gitDiffGutter, setGitBaseline } from "./gitGutter";
import { diffApi } from "../api/diff";

afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
});

it("cancels delayed Git diff work when its document is replaced", async () => {
    vi.useFakeTimers();
    const hunks = vi.spyOn(diffApi, "hunks").mockResolvedValue([]);
    const view = new EditorView({ state: EditorState.create({ doc: "changed", extensions: gitDiffGutter() }) });
    setGitBaseline(view, "original");
    view.setState(EditorState.create({ doc: "next", extensions: gitDiffGutter() }));
    setGitBaseline(view, "next original");
    await vi.advanceTimersByTimeAsync(500);
    expect(hunks).toHaveBeenCalledTimes(1);
    expect(hunks).toHaveBeenCalledWith("next original", "next");
    view.destroy();
});
