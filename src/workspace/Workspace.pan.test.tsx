import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Workspace } from "./Workspace";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { performanceTelemetry } from "../lib/performance";
import { HELD_END_MS, SPENT_END_MS } from "../lib/swipe";
import { setFingersDown } from "../lib/wheelTouch";
import { PAN_MS } from "./useWindowPan";
import type { Agent } from "../state/types";

vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div>Terminal output</div> }));
vi.mock("../chat/AgentSurface", () => ({ AgentSurface: () => <div>Agent output</div> }));
vi.mock("./BrowserPane", () => ({ AgentBrowserShell: ({ children }: { children: React.ReactNode }) => children }));
vi.mock("../editor/EditorPane", () => ({ EditorPane: () => <div>Editor document</div> }));

const initial = getState();

beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
    setFingersDown(false);
    performanceTelemetry.reset();
});
afterEach(cleanup);

const SCREENS = 12;

/** A session whose screens are all mounted at once: a live agent keeps its layer wherever it is. */
function sessionOfScreens(): void {
    const state = getState();
    const sessionId = state.activeSessionId;
    const agents: Agent[] = Array.from({ length: SCREENS }, (_, index) => ({
        id: `agent-${index}`,
        type: "codex",
        title: `agent ${index}`,
        startup: "codex",
        launchState: "live",
    }));
    const slices = withAgents(state, sessionId, agents);
    setState({
        ...slices,
        sessions: {
            ...state.sessions,
            [sessionId]: { ...state.sessions[sessionId], kind: "project", cwd: "/repo", activeWindowId: agentWindowId(slices, "agent-0")! },
        },
    });
}

const slotOf = (layer: Element) => Number((layer as HTMLElement).style.getPropertyValue("--slot"));

/** The track's transform once it has been slid `screens` screens to the left. */
const slidLeft = (screens: number) => `translate(calc(${-screens} * (100% + var(--window-card-gap))), 0)`;

describe("workspace pan", () => {
    /*
     * The whole cost claim of the pan is here: a jump across a session paints the
     * screen being left and the screen arriving, and nothing in between, however
     * many screens the session has and however far apart the two are.
     */
    it("paints two layers for a far jump and one once it settles", async () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        await act(async () => {});
        expect(container.querySelectorAll(".window-layer").length).toBeGreaterThanOrEqual(SCREENS);

        const sessionId = getState().activeSessionId;
        const to = agentWindowId(getState(), "agent-9")!;
        const homeSlot = getState().windowsBySession[sessionId].indexOf(to);
        act(() => cmd.selectWindowId(to));

        const painted = container.querySelectorAll(".window-layer.painted");
        expect(painted).toHaveLength(2);
        const track = container.querySelector(".window-track") as HTMLElement;
        expect(track).toHaveClass("panning");
        // A switch travels a whole screen, so it takes a whole screen's time.
        expect(track.style.getPropertyValue("--window-pan-ms")).toBe(`${PAN_MS}ms`);

        // Parked beside the screen being left rather than nine screens away, so
        // the track travels one screen either way.
        const slots = Array.from(painted, slotOf).sort((left, right) => left - right);
        expect(slots[1] - slots[0]).toBe(1);
        expect(slots).not.toContain(homeSlot);

        await waitFor(() => expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1));
        expect(container.querySelector(".window-track")).not.toHaveClass("panning");
        expect(slotOf(container.querySelector(".window-layer.painted")!)).toBe(homeSlot);
    });

    /* The texture belongs to the panes now rather than to the screen, and only
       to the panes on the screen being read — a field on a screen off stage is
       a WebGL context taken from a terminal that needs one. */
    it("gives the texture to the panes on stage and to no other", async () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        await act(async () => {});

        expect(container.querySelectorAll(".screen-field")).toHaveLength(0);
        const live = () => container.querySelector(".window-layer.live")!;
        expect(live().querySelectorAll(".pane-field").length).toBe(live().querySelectorAll(".pane").length);
        expect(live().querySelectorAll(".pane-field").length).toBeGreaterThan(0);

        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-9")!));
        await waitFor(() => expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1));

        expect(live().querySelectorAll(".pane-field").length).toBe(live().querySelectorAll(".pane").length);
    });

    /*
     * Holding the switch shortcut down starts the next slide while the last one is
     * still travelling. The window it leaves behind is the one that slide parked
     * next door, so the next slide has to start from there and not from that
     * window's own screen, or the canvas crosses everything in between.
     */
    it("travels one screen for a switch made while a slide is running", async () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        await act(async () => {});

        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-9")!));
        const parked = slotOf(container.querySelector(".window-layer.live")!);

        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-11")!));

        const painted = container.querySelectorAll(".window-layer.painted");
        expect(painted).toHaveLength(2);
        const slots = Array.from(painted, slotOf).sort((left, right) => left - right);
        expect(slots[1] - slots[0]).toBe(1);
        expect(slots).toContain(parked);

        const track = container.querySelector(".window-track") as HTMLElement;
        expect(track).toHaveClass("panning");
        expect(track.style.transform).toBe(slidLeft(parked + 1));
    });

    /*
     * Chained slides walk the parked screen along one step at a time, and stepping
     * left far enough takes it past the track's own left edge. The offset has to
     * follow it there or the screen arriving never reaches the stage.
     */
    it("parks a screen left of the track when the chain keeps stepping left", async () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        await act(async () => {});
        const track = container.querySelector(".window-track") as HTMLElement;
        const parkedAt = () => slotOf(container.querySelector(".window-layer.live")!);

        for (const agent of ["agent-9", "agent-4", "agent-1", "agent-0"]) {
            act(() => cmd.selectWindowId(agentWindowId(getState(), agent)!));
            expect(track.style.transform).toBe(slidLeft(parkedAt()));
        }

        expect(parkedAt()).toBe(-1);
    });

    /*
     * The store commits the new window before the slide starts, so the tab pill and
     * the keyboard are already on the target. The layer being left must therefore be
     * out of the a11y tree and unfocusable from the first frame, even while it paints.
     */
    it("hands the target focus and inertness the moment the pan starts", () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);

        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-1")!));

        const outgoing = container.querySelectorAll(".window-layer.painted:not(.live)");
        expect(outgoing).toHaveLength(1);
        expect(outgoing[0]).toHaveAttribute("inert");
        expect(outgoing[0]).toHaveAttribute("aria-hidden", "true");
        const incoming = container.querySelectorAll(".window-layer.live");
        expect(incoming).toHaveLength(1);
        expect(incoming[0]).not.toHaveAttribute("inert");
    });

    it("records a tab-pan span", async () => {
        sessionOfScreens();
        render(<Workspace />);
        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-3")!));

        await waitFor(() => expect(performanceTelemetry.snapshot().spans.map((span) => span.name)).toContain("tab-pan"));
        const span = performanceTelemetry.snapshot().spans.find((entry) => entry.name === "tab-pan")!;
        expect(span.metadata).toMatchObject({ distance: 3, teleported: true });
    });

    /*
     * Two documents of one editor are one screen, not two, so the canvas has
     * nowhere to slide to.
     */
    it("does not pan when the document changes inside one window", () => {
        cmd.requestOpenFile("/repo/a.ts");
        cmd.requestOpenFile("/repo/b.ts");
        const { container } = render(<Workspace />);
        const editorWindow = getState().sessions[getState().activeSessionId].activeWindowId;

        act(() => cmd.selectTab({ id: editorWindow, doc: "/repo/a.ts" }));

        expect(container.querySelector(".window-track")).not.toHaveClass("panning");
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1);
    });

    it("cuts straight through when motion is reduced", () => {
        vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)", media: query }));
        sessionOfScreens();
        const { container } = render(<Workspace />);

        act(() => cmd.selectWindowId(agentWindowId(getState(), "agent-5")!));

        expect(container.querySelector(".window-track")).not.toHaveClass("panning");
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1);
        vi.unstubAllGlobals();
    });
});

describe("workspace wheel pan", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    const STAGE_WIDTH = 1000;
    /** Longer than any landing takes. */
    const LANDING_MS = 700;
    const activeWindow = () => getState().sessions[getState().activeSessionId].activeWindowId;
    const order = () => getState().windowsBySession[getState().activeSessionId];

    /** jsdom lays nothing out, so the stage has to be told how wide a screen is. */
    function stageOfScreens(width = STAGE_WIDTH): { track: HTMLElement; live: HTMLElement; index: number; container: HTMLElement } {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        Object.defineProperty(container.querySelector(".window-area")!, "clientWidth", { value: width, configurable: true });
        return {
            container,
            track: container.querySelector(".window-track") as HTMLElement,
            live: container.querySelector(".window-layer.live") as HTMLElement,
            index: order().indexOf(activeWindow()),
        };
    }

    /** One wheel event, plus the frame the track is written on. */
    const swipe = (over: Element, deltaX: number, deltaY = 0) => {
        const taken = !fireEvent.wheel(over, { deltaX, deltaY });
        act(() => void vi.advanceTimersByTime(20));
        return taken;
    };
    /** A hand still moving fast as it lifts, having pulled the next screen hardly on at all. */
    const flick = (over: Element) => {
        swipe(over, 20);
        swipe(over, 20);
        swipe(over, 20);
    };
    const lift = () => act(() => setFingersDown(false));
    const landed = () => act(() => void vi.advanceTimersByTime(LANDING_MS));
    const panOf = (track: HTMLElement) => track.style.transform;
    const pulled = (track: HTMLElement) => -Number.parseFloat(panOf(track).slice("translate(calc(".length));

    /*
     * The finger drives the track directly: the offsets are on, the screen it is
     * heading for paints beside the one on stage, and nothing transitions while
     * the gesture is still going.
     */
    it("follows the finger and animates nothing while the gesture is live", () => {
        const { container, track, live, index } = stageOfScreens();

        expect(swipe(live, 100)).toBe(true);
        swipe(live, 100);

        expect(track).toHaveClass("panning");
        expect(track).not.toHaveClass("sliding");
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(2);
        expect(panOf(track)).toBe(slidLeft(index + 0.2));
    });

    /*
     * One swipe is one screen. However far the hand pulls, the session stays put
     * while it is down, the track stops at the next screen, and nothing past it paints.
     */
    it("moves no further than the next screen while the hand is down", () => {
        const { container, track, live, index } = stageOfScreens();
        const before = activeWindow();
        setFingersDown(true);

        for (let i = 0; i < 6; i++) swipe(live, 300);

        expect(activeWindow()).toBe(before);
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(2);
        expect(pulled(track) - index).toBeGreaterThan(1);
        expect(pulled(track) - index).toBeLessThan(1.15);
    });

    /*
     * The landing is the track's alone. The session moves once the track has
     * arrived, so nothing re-renders while anything is moving.
     */
    it("lands on the screen a pull was set down on, and only then moves the session", () => {
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];
        setFingersDown(true);

        swipe(live, 400);
        act(() => void vi.advanceTimersByTime(200));
        lift();

        expect(activeWindow()).not.toBe(neighbour);
        expect(track).toHaveClass("panning");

        landed();

        expect(activeWindow()).toBe(neighbour);
        expect(track).not.toHaveClass("panning");
        expect(panOf(track)).toBe("");
    });

    /*
     * Holding still mid-swipe sends nothing at all, which is exactly what having
     * let go sends. macOS is the only thing that can tell the two apart, and
     * while it says the hand is down the track stays where the hand left it
     * however long the pause runs.
     */
    it("keeps following a finger that holds still part way through", () => {
        const { track, live, index } = stageOfScreens();
        setFingersDown(true);
        const pause = HELD_END_MS - 100;

        swipe(live, 300);
        act(() => void vi.advanceTimersByTime(pause));

        expect(track).toHaveClass("panning");
        expect(panOf(track)).toBe(slidLeft(index + 0.3));

        swipe(live, 300);
        act(() => void vi.advanceTimersByTime(pause));

        expect(panOf(track)).toBe(slidLeft(index + 0.6));
    });

    /* A pull set down short of a quarter of the next screen has not chosen it. */
    it("puts back a pull that never got far onto the next screen", () => {
        const { track, live } = stageOfScreens();
        const before = activeWindow();

        swipe(live, 200);
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));
        landed();

        expect(activeWindow()).toBe(before);
        expect(track).not.toHaveClass("panning");
    });

    /*
     * A hand still moving fast as it lifts has thrown the swipe, and it carries on
     * to the next screen however little of it was showing.
     */
    it("carries a thrown swipe on to the next screen", () => {
        const { live, index } = stageOfScreens();
        setFingersDown(true);

        flick(live);
        lift();
        landed();

        expect(activeWindow()).toBe(order()[index + 1]);
    });

    /*
     * A hand does not move further because the screen it is on is wider, so the
     * same throw has to carry on a big monitor.
     */
    it("carries the same throw however wide the screen is", () => {
        const { live, index } = stageOfScreens(3000);
        setFingersDown(true);

        flick(live);
        lift();
        landed();

        expect(activeWindow()).toBe(order()[index + 1]);
    });

    /* The same distance, set down rather than thrown, was never going anywhere. */
    it("puts back a swipe the hand had stopped before it left", () => {
        const { live } = stageOfScreens();
        const before = activeWindow();
        setFingersDown(true);

        flick(live);
        act(() => void vi.advanceTimersByTime(400));
        lift();
        landed();

        expect(activeWindow()).toBe(before);
    });

    /* A hand pulling back as it lifts has changed its mind, however far it had got. */
    it("puts back a swipe the hand was taking back as it lifted", () => {
        const { live } = stageOfScreens();
        const before = activeWindow();
        setFingersDown(true);

        swipe(live, 400);
        swipe(live, -30);
        swipe(live, -30);
        lift();
        landed();

        expect(activeWindow()).toBe(before);
    });

    /*
     * A trackpad keeps sending the swipe's glide for a second or more after the
     * hand has gone. The swipe is landing by then, so the glide may not drag the
     * track anywhere else.
     */
    it("ignores the glide a swipe leaves behind", () => {
        const { track, live, index } = stageOfScreens();
        setFingersDown(true);
        swipe(live, 400);
        lift();

        expect(swipe(live, 600)).toBe(true);
        swipe(live, 600);
        landed();

        expect(activeWindow()).toBe(order()[index + 1]);
        expect(panOf(track)).toBe("");
    });

    /* And a hand coming back down mid-glide is a new swipe, counted from where the last one landed. */
    it("takes a hand coming back down as a new swipe", () => {
        const { track, live, index } = stageOfScreens();
        setFingersDown(true);
        swipe(live, 400);
        lift();
        landed();
        swipe(live, 400);

        act(() => setFingersDown(true));
        swipe(live, 300);

        expect(panOf(track)).toBe(slidLeft(index + 1.3));
    });

    /* A swipe started while the last one is still landing finishes that landing first. */
    it("starts a new swipe from where a landing was going", () => {
        const { track, live, index } = stageOfScreens();
        setFingersDown(true);
        swipe(live, 400);
        lift();

        act(() => setFingersDown(true));
        swipe(live, 100);

        expect(activeWindow()).toBe(order()[index + 1]);
        expect(panOf(track)).toBe(slidLeft(index + 1.1));
    });

    /*
     * There is nothing beyond the last screen to pull on, so the pull gives a
     * little and then stops giving, and no screen is ever handed over.
     */
    it("resists rather than scrolls at the end of the session", () => {
        const { container, track, live } = stageOfScreens();
        const last = order().at(-1)!;
        act(() => cmd.selectWindowId(last));
        act(() => void vi.advanceTimersByTime(PAN_MS * 2));
        const index = order().indexOf(last);

        swipe(live, 900);
        swipe(live, 900);

        expect(activeWindow()).toBe(last);
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(1);
        expect(pulled(track) - index).toBeGreaterThan(0);
        expect(pulled(track) - index).toBeLessThan(0.15);

        act(() => void vi.advanceTimersByTime(SPENT_END_MS));
        landed();

        expect(activeWindow()).toBe(last);
        expect(track).not.toHaveClass("panning");
    });

    /*
     * Whether the stage takes a gesture is decided on its first event and kept
     * for the rest of it, so scrolling a wide pane to its edge does not throw the
     * tail of the same swipe at the stage.
     */
    it("leaves a swipe to the pane under it, even after that pane runs out of room", () => {
        const { track, live } = stageOfScreens();
        const before = activeWindow();
        const pane = live.appendChild(document.createElement("div"));
        pane.style.overflowX = "auto";
        Object.defineProperty(pane, "scrollWidth", { value: 800, configurable: true });
        Object.defineProperty(pane, "clientWidth", { value: 300, configurable: true });
        Object.defineProperty(pane, "scrollLeft", { value: 0, writable: true, configurable: true });

        expect(swipe(pane, 300)).toBe(false);
        pane.scrollLeft = 500;
        expect(swipe(pane, 300)).toBe(false);
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(track).not.toHaveClass("panning");
        expect(activeWindow()).toBe(before);
    });

    /*
     * A switch from the keyboard mid-swipe takes the session off the screens the
     * gesture was dragging between, so the gesture has nothing left to hold and
     * its landing must not pull the session back.
     */
    it("lets go when the session is switched out from under it", () => {
        const { live, index } = stageOfScreens();
        const chosen = order()[index + 4];

        swipe(live, 400);
        act(() => cmd.selectWindowId(chosen));
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));
        landed();

        expect(activeWindow()).toBe(chosen);
    });

    /*
     * A screen closing elsewhere moves the session along the track without
     * switching it, so no switch plans a slide. The track still has to come back,
     * or the whole session sits half a screen off the stage.
     */
    it("gives the track back when the screens move under the swipe", () => {
        const { container, track } = stageOfScreens();
        act(() => cmd.selectWindowId(order()[3]));
        act(() => void vi.advanceTimersByTime(PAN_MS * 2));
        const on = activeWindow();

        swipe(container.querySelector(".window-layer.live")!, 200);
        expect(track).toHaveClass("panning");

        act(() => cmd.closeWindowById(order()[0]));
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));
        landed();

        expect(activeWindow()).toBe(on);
        expect(track).not.toHaveClass("panning");
        expect(panOf(track)).toBe("");
    });

    /*
     * Dragging is direct manipulation rather than animation, so it still follows
     * the finger with motion reduced; only the landing stops being a movement.
     */
    it("follows the finger with motion reduced and cuts at the end", () => {
        vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)", media: query }));
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];

        swipe(live, 600);
        expect(track).toHaveClass("panning");
        expect(panOf(track)).toBe(slidLeft(index + 0.6));

        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(activeWindow()).toBe(neighbour);
        expect(track).not.toHaveClass("panning");
        expect(panOf(track)).toBe("");
        vi.unstubAllGlobals();
    });
});

describe("window scroll indicator", () => {
    const sessionId = () => getState().activeSessionId;
    const order = () => getState().windowsBySession[sessionId()];

    /*
     * The thumb says where the window really is in the session. A far jump parks
     * the canvas next door and slides one screen, and the thumb has to cross the
     * whole gap regardless, or it would report the parked screen as the place.
     */
    it("sizes the thumb by screen count and places it by the real index", () => {
        sessionOfScreens();
        const { container } = render(<Workspace />);
        const thumb = () => container.querySelector(".window-scroll-thumb") as HTMLElement;
        const count = order().length;

        expect(thumb().style.width).toBe(`${100 / count}%`);
        expect(thumb().style.transform).toBe(`translateX(${order().indexOf(getState().sessions[sessionId()].activeWindowId) * 100}%)`);

        const to = agentWindowId(getState(), "agent-9")!;
        const index = order().indexOf(to);
        act(() => cmd.selectWindowId(to));

        expect(thumb().style.transform).toBe(`translateX(${index * 100}%)`);
        expect(slotOf(container.querySelector(".window-layer.live")!)).not.toBe(index);
    });

    it("has nothing to show for a session of one screen", () => {
        const { container } = render(<Workspace />);

        expect(order()).toHaveLength(1);
        expect(container.querySelector(".window-scroll")).toBeNull();
    });
});
