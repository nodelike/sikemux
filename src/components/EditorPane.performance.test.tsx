import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { EditorPane } from "./EditorPane";
import { getState, setState } from "../state/store";
import { emit } from "../state/bus";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();
const initial = getState();
afterEach(() => {
    cleanup();
    setState(initial, true);
});

it("stress switches 12 warm documents 200 times without eager-loading inactive restored tabs", async () => {
    const paths = Array.from({ length: 12 }, (_, i) => `/repo/file-${i}.ts`);
    const content = Array.from({ length: 1_000 }, (_, i) => `export const value${i} = ${i};`).join("\n");
    invoke.mockImplementation(async (command: string) => {
        if (command === "read_file_versioned") return { content, version: "1" };
        if (command === "read_file") return content;
        if (command === "repo_watch_start") return 1;
        if (command === "diff_hunks") return [];
        return null;
    });
    setState({ editorViews: { pane: { openTabs: paths, activePath: paths[0] } } });
    const { container } = render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} onCloseWindow={() => {}} />);
    await waitFor(() => expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned")).toHaveLength(1));
    const tabs = [...container.querySelectorAll<HTMLElement>('[role="tab"]')];
    expect(tabs).toHaveLength(12);
    for (let index = 1; index < tabs.length; index++) {
        act(() => setState({ editorViews: { pane: { openTabs: paths, activePath: paths[index] } } }));
        await waitFor(() => expect(getState().editorViews.pane.activePath).toBe(paths[index]));
        await waitFor(() => expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned")).toHaveLength(index + 1));
    }
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned")).toHaveLength(12);
    const samples: number[] = [];
    for (let i = 0; i < 200; i++) {
        const index = (i + 1) % tabs.length;
        const start = performance.now();
        fireEvent.click(tabs[index]);
        samples.push(performance.now() - start);
        expect(getState().editorViews.pane.activePath).toBe(paths[index]);
    }
    samples.sort((a, b) => a - b);
    process.stdout.write(JSON.stringify({ switches: samples.length, medianMs: samples[100], p95Ms: samples[190], maxMs: samples[199] }) + "\n");
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned")).toHaveLength(12);
    act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["file-4.ts"] }));
    await waitFor(() => expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned")).toHaveLength(13));
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "read_file_versioned").at(-1)?.[1]).toEqual({ path: paths[4] });
}, 30_000);
