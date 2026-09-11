import { act, renderHook } from "@testing-library/react";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { expect, it, vi } from "vitest";
import { git } from "../api/git";
import * as gutter from "../editor/gitGutter";
import { useGitBaseline } from "./useGitBaseline";

it("ignores a previous file's baseline after switching the shared editor", async () => {
    const responses: Array<(value: string) => void> = [];
    const fileAt = vi.spyOn(git, "fileAt").mockImplementation(() => new Promise<string>((resolve) => responses.push(resolve)));
    const baseline = vi.spyOn(gutter, "setGitBaseline").mockImplementation(() => {});
    const view = new EditorView({ state: EditorState.create({ doc: "current" }) });
    const hook = renderHook(({ path }) => useGitBaseline(() => view, "/repo", path), { initialProps: { path: "/repo/first.ts" } });
    hook.rerender({ path: "/repo/second.ts" });
    await act(async () => {
        responses[0]("stale");
    });
    expect(baseline).not.toHaveBeenCalled();
    await act(async () => {
        responses[1]("current baseline");
    });
    expect(baseline).toHaveBeenCalledExactlyOnceWith(view, "current baseline");
    hook.unmount();
    view.destroy();
    baseline.mockRestore();
    fileAt.mockRestore();
});
