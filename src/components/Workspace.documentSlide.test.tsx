import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { Workspace } from "./Workspace";
import type { Agent } from "../state/types";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div>Terminal output</div> }));
vi.mock("../chat/AgentSurface", () => ({ AgentSurface: () => <div>Agent output</div> }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));

Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

const initial = getState();
const FIRST = "/repo/a.ts";
const SECOND = "/repo/b.ts";
const THIRD = "/repo/c.ts";

beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
    invoke.mockImplementation(async (command: string) => (command === "read_file_versioned" ? { content: "one\ntwo\n", version: "v1" } : null));
});
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

/** An editor window on the stage with three documents open, the first of them live. */
async function editorOfDocuments(): Promise<{ container: HTMLElement; paneId: string }> {
    const before = getState();
    setState({
        sessions: { ...before.sessions, [before.activeSessionId]: { ...before.sessions[before.activeSessionId], kind: "project", cwd: "/repo" } },
    });
    cmd.requestOpenFile(FIRST);
    const state = getState();
    const paneId = state.windows[state.sessions[state.activeSessionId].activeWindowId].activePaneId;
    setState({ editorViews: { [paneId]: { openTabs: [FIRST, SECOND, THIRD], activePath: FIRST } } });
    const { container } = render(<Workspace />);
    // The first editor in a file loads CodeMirror cold, which on CI can take over the default second.
    await waitFor(() => expect(container.querySelector(".cm-content")).toHaveTextContent("one"), { timeout: 5_000 });
    return { container, paneId };
}

const select = (doc: string) => {
    const windowId = getState().sessions[getState().activeSessionId].activeWindowId;
    act(() => cmd.selectTab({ id: windowId, doc }));
};

const snapshots = (container: HTMLElement) => container.querySelectorAll(".doc-snapshot");
const transformOf = (element: Element) => (element as HTMLElement).style.transform;
const hostOf = (container: HTMLElement) => container.querySelector(".ed-host") as HTMLElement;
const endSlide = (host: HTMLElement) => act(() => void host.dispatchEvent(new TransitionEvent("transitionend", { propertyName: "transform" })));

/** A media query list complete enough for the editor's own theme listeners. */
const reduceMotion = () =>
    vi.stubGlobal("matchMedia", (query: string) => ({
        matches: query === "(prefers-reduced-motion: reduce)",
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false,
        onchange: null,
    }));

describe("document slide", () => {
    /*
     * The document leaving is a copy taken before the swap, not a second editor:
     * the live host keeps the one `EditorView` and does the arriving. The copy
     * must be unreachable while it travels — no click, no focus, no screen reader.
     */
    it("slides an inert copy of the document out and the live one in", async () => {
        const { container } = await editorOfDocuments();

        select(SECOND);

        const copy = snapshots(container);
        expect(copy).toHaveLength(1);
        expect(copy[0]).toHaveAttribute("inert");
        expect(copy[0]).toHaveAttribute("aria-hidden", "true");
        expect(container.querySelectorAll(".cm-editor")).toHaveLength(2);
        // The copy carries CodeMirror's own classes, so the stylesheet in the
        // document head dresses it exactly like the editor it came from.
        expect(copy[0].querySelector(".cm-editor")).toBeInTheDocument();
        expect(copy[0].querySelector(".cm-content")).toHaveTextContent("one");

        const host = container.querySelector(".ed-host.doc-slide-in") as HTMLElement;
        expect(transformOf(host)).toBe("translate3d(0%, 0, 0)");
        expect(host.parentElement).toHaveClass("doc-sliding", "doc-travelling");
        expect(transformOf(copy[0])).toBe("translate3d(-100%, 0, 0)");
    });

    it("leaves nothing of the slide behind once it settles", async () => {
        const { container } = await editorOfDocuments();

        select(SECOND);
        const host = hostOf(container);
        endSlide(host);

        expect(snapshots(container)).toHaveLength(0);
        expect(host).not.toHaveClass("doc-slide-in");
        expect(host.getAttribute("style")).not.toContain("transform");
        expect(host.parentElement).not.toHaveClass("doc-sliding");
    });

    /*
     * `cloneNode` carries no scroll position, so a copy of a document read down to
     * line four hundred would otherwise jump back to its first line as it leaves.
     */
    it("carries the scroll position of the document leaving onto its copy", async () => {
        const { container } = await editorOfDocuments();
        const scroller = hostOf(container).querySelector(".cm-scroller") as HTMLElement;
        scroller.scrollTop = 240;
        scroller.scrollLeft = 30;

        select(SECOND);

        const copy = container.querySelector(".doc-snapshot .cm-scroller") as HTMLElement;
        expect(copy.scrollTop).toBe(240);
        expect(copy.scrollLeft).toBe(30);
    });

    /*
     * Which way a document travels is its place in the strip's own list, so going
     * back up the list brings the document in from the left.
     */
    it("takes its direction from the order of the documents", async () => {
        const { container } = await editorOfDocuments();

        select(THIRD);
        expect(transformOf(container.querySelector(".doc-snapshot")!)).toBe("translate3d(-100%, 0, 0)");

        endSlide(hostOf(container));
        select(FIRST);

        expect(transformOf(container.querySelector(".doc-snapshot")!)).toBe("translate3d(100%, 0, 0)");
    });

    /*
     * Holding the shortcut down switches again while the last slide is still
     * travelling. Each link of the chain is one screen, so the copy that was
     * travelling goes and the document it was bringing in becomes the next copy.
     */
    it("keeps one copy through a switch made mid-slide", async () => {
        const { container } = await editorOfDocuments();

        select(SECOND);
        const first = container.querySelector(".doc-snapshot");
        select(THIRD);

        const copy = snapshots(container);
        expect(copy).toHaveLength(1);
        expect(copy[0]).not.toBe(first);
        expect(first).not.toBeInTheDocument();
    });

    it("cuts straight to the document when motion is reduced", async () => {
        reduceMotion();
        const { container } = await editorOfDocuments();

        select(SECOND);

        expect(snapshots(container)).toHaveLength(0);
    });

    /*
     * Reaching a document of a window the session is not on is the track's travel,
     * and the screen arriving brings its new document with it. A copy of the
     * document it left would slide inside a screen that is already sliding.
     */
    it("takes no copy when the document arrives with its own screen", async () => {
        const { container } = await editorOfDocuments();
        const editorWindow = getState().sessions[getState().activeSessionId].activeWindowId;
        const agents: Agent[] = [{ id: "agent-0", type: "codex", title: "agent 0", startup: "codex", launchState: "live" }];
        act(() => setState(withAgents(getState(), getState().activeSessionId, agents)));
        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-0")!));
        endSlide(container.querySelector(".window-track") as HTMLElement);

        act(() => cmd.selectTab({ id: editorWindow, doc: SECOND }));

        expect(snapshots(container)).toHaveLength(0);
        expect(container.querySelector(".window-track")).toHaveClass("panning");
    });

    /*
     * Two documents of one screen are one screen, so the canvas has nothing to do
     * while the document under it changes.
     */
    it("does not pan the track", async () => {
        const { container } = await editorOfDocuments();

        select(SECOND);

        expect(container.querySelector(".window-track")).not.toHaveClass("panning");
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1);
        expect(snapshots(container)).toHaveLength(1);
    });
});
