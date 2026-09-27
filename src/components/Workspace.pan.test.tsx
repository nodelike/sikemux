import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Workspace } from "./Workspace";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { performanceTelemetry } from "../lib/performance";
import { HELD_END_MS, SPENT_END_MS } from "./wheelPan";
import { setFingersDown } from "../lib/wheelTouch";
import { PAN_MS, RETURN_MS, settleMs } from "./useWindowPan";
import type { Agent } from "../state/types";

vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div>Terminal output</div> }));
vi.mock("../chat/AgentSurface", () => ({ AgentSurface: () => <div>Agent output</div> }));
vi.mock("./BrowserPane", () => ({ AgentBrowserShell: ({ children }: { children: React.ReactNode }) => children }));
vi.mock("./EditorPane", () => ({ EditorPane: () => <div>Editor document</div> }));

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

/** What `--pan` reads once the track has been slid `screens` screens to the left. */
const slidLeft = (screens: number) => `calc(${-screens} * (100% + var(--window-card-gap)))`;

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
        expect(track.style.getPropertyValue("--pan")).toBe(slidLeft(parked + 1));
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
            expect(track.style.getPropertyValue("--pan")).toBe(slidLeft(parkedAt()));
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
    const settleTime = (track: HTMLElement) => Number.parseFloat(track.style.getPropertyValue("--window-pan-ms"));
    const panOf = (track: HTMLElement) => track.style.getPropertyValue("--pan");

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
     * The strip, the keyboard and the indicator follow the screen the swipe has
     * pulled more than halfway on. The track is not told anything: it is still
     * under the finger, in exactly the place the last frame left it.
     */
    it("makes the screen more than half on the active one without moving the track", () => {
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];

        swipe(live, 400);
        expect(activeWindow()).not.toBe(neighbour);
        expect(panOf(track)).toBe(slidLeft(index + 0.4));

        swipe(live, 200);

        expect(activeWindow()).toBe(neighbour);
        expect(track).not.toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 0.6));
    });

    /*
     * The events a trackpad keeps sending after the fingers leave are the swipe
     * carrying on, so they keep scrolling rather than being swallowed. Each
     * screen crossed becomes the one the drag counts from, and at every moment
     * the finger is between exactly two of them.
     */
    it("scrolls on through the screens after it for as long as the events come", () => {
        const { container, track, live, index } = stageOfScreens();
        const painted = () => container.querySelectorAll(".window-layer.painted").length;

        swipe(live, 600);
        expect(activeWindow()).toBe(order()[index + 1]);
        expect(painted()).toBe(2);

        swipe(live, 600);
        expect(activeWindow()).toBe(order()[index + 1]);
        expect(painted()).toBe(2);

        swipe(live, 600);

        expect(activeWindow()).toBe(order()[index + 2]);
        expect(painted()).toBe(2);
        expect(panOf(track)).toBe(slidLeft(index + 1.8));
    });

    /*
     * Quiet is the only thing that animates, and all it ever has to cover is the
     * half screen between the finger and whatever screen the swipe ended on.
     */
    it("closes onto the active screen once the events stop", () => {
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];

        swipe(live, 600);
        expect(track).not.toHaveClass("sliding");

        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(activeWindow()).toBe(neighbour);
        expect(track).toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 1));
        expect(settleTime(track)).toBeGreaterThan(0);
        expect(settleTime(track)).toBeLessThan(PAN_MS);
    });

    /*
     * A hand that has gone is not going to pull any further, so a pull set down
     * well onto the next screen has chosen it even though it never reached the
     * half a screen a crossing takes under a hand that is still there.
     */
    it("takes the screen a pull was set down most of the way onto", () => {
        const { track, live, index } = stageOfScreens();

        swipe(live, 350);
        expect(activeWindow()).toBe(order()[index]);

        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(activeWindow()).toBe(order()[index + 1]);
        expect(panOf(track)).toBe(slidLeft(index + 1));
    });

    /*
     * A pull that has already crossed onto the next screen is counted from that
     * one, and the ground that carried it there is the same ground a throw is read
     * from, so the throw may not buy a second screen with it. Landing two screens
     * on would also travel across a screen that nobody painted.
     */
    it("does not let a throw spend the ground the crossing already did", () => {
        const { container, track, live, index } = stageOfScreens();

        act(() => setFingersDown(true));
        swipe(live, 600);
        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(order()[index + 1]);
        expect(container.querySelectorAll(".window-layer.painted")).toHaveLength(2);
        expect(panOf(track)).toBe(slidLeft(index + 1));
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
        // Far longer than a person pauses, and still not the end of the swipe.
        const pause = HELD_END_MS - 100;

        swipe(live, 300);
        act(() => void vi.advanceTimersByTime(pause));

        expect(track).not.toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 0.3));

        swipe(live, 300);
        act(() => void vi.advanceTimersByTime(pause));

        expect(track).not.toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 0.6));
    });

    /*
     * The hand leaving is the end of the swipe, so the track closes there and
     * then. Waiting for the glide the hand left to run out would hold it still
     * for the second or two that takes.
     */
    it("closes the moment the hand leaves, with nothing left to wait for", () => {
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];
        setFingersDown(true);

        swipe(live, 600);
        act(() => void vi.advanceTimersByTime(400));
        expect(track).not.toHaveClass("sliding");

        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(neighbour);
        expect(track).toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 1));
    });

    /*
     * Nothing carries a swipe once the hand has gone, so a swipe still moving
     * when it ended has to carry itself: it goes on to the next screen even
     * though the hand never pulled it halfway on.
     */
    it("carries a thrown swipe on to the next screen", () => {
        const { track, live, index } = stageOfScreens();
        const neighbour = order()[index + 1];
        setFingersDown(true);

        swipe(live, 200);
        expect(activeWindow()).not.toBe(neighbour);

        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(neighbour);
        expect(track).toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index + 1));
    });

    /*
     * A hand does not move further because the screen it is on is wider, so the
     * same throw has to carry on a big monitor. Measured against the screen it
     * would not: this is a fifteenth of a wide stage and over half of a narrow one.
     */
    it("carries the same throw however wide the screen is", () => {
        const { track, live, index } = stageOfScreens(3000);
        const neighbour = order()[index + 1];
        setFingersDown(true);

        swipe(live, 200);
        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(neighbour);
        expect(panOf(track)).toBe(slidLeft(index + 1));
    });

    /* The same distance, set down rather than thrown, was never going anywhere. */
    it("puts back a swipe the hand had stopped before it left", () => {
        const { track, live, index } = stageOfScreens();
        const before = activeWindow();
        setFingersDown(true);

        swipe(live, 200);
        act(() => void vi.advanceTimersByTime(400));
        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(before);
        expect(panOf(track)).toBe(slidLeft(index));
    });

    /*
     * A trackpad keeps sending the swipe's glide for a second or more after the
     * hand has gone. The swipe has already landed by then, so the glide may not
     * start dragging the screen it landed on off again.
     */
    it("ignores the glide a swipe leaves behind", () => {
        const { track, live } = stageOfScreens();
        setFingersDown(true);
        swipe(live, 600);
        act(() => setFingersDown(false));

        const landed = activeWindow();
        const at = panOf(track);
        expect(swipe(live, 600)).toBe(true);
        swipe(live, 600);

        expect(activeWindow()).toBe(landed);
        expect(panOf(track)).toBe(at);
    });

    /* And a hand coming back down mid-glide is a new swipe, not more of the old one.
       The first one crossed onto the screen after it and closed there, so the second
       is a fresh 0.3 of a screen from that one and not from where the glide went. */
    it("takes a hand coming back down as a new swipe", () => {
        const { track, live, index } = stageOfScreens();
        setFingersDown(true);
        swipe(live, 600);
        act(() => setFingersDown(false));
        swipe(live, 400);

        act(() => setFingersDown(true));
        swipe(live, 300);

        expect(panOf(track)).toBe(slidLeft(index + 1.3));
    });

    /*
     * A swipe that never pulled a screen halfway on has not chosen it, so quiet
     * puts the one it started on back. This is the shortest travel there is, and
     * it may not crawl.
     */
    it("puts the screen back when the swipe never pulled the next one halfway on", () => {
        const { track, live, index } = stageOfScreens();
        const before = activeWindow();

        swipe(live, 100);
        expect(track).not.toHaveClass("sliding");

        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(activeWindow()).toBe(before);
        expect(track).toHaveClass("sliding");
        expect(panOf(track)).toBe(slidLeft(index));
        expect(settleTime(track)).toBeLessThan(PAN_MS / 2);
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
        const pulled = Number.parseFloat(panOf(track).slice("calc(".length));
        expect(-pulled - index).toBeGreaterThan(0);
        expect(-pulled - index).toBeLessThan(0.15);
    });

    /*
     * A swipe thrown at a screen that is not there is the fastest the track ever
     * goes, and the close has to turn it round. Coming back is its own movement:
     * one time for every one of them, whatever the throw was worth, rather than a
     * time read off the little ground a resisted pull covered.
     */
    it("brings a swipe thrown past the last screen back in the time a swipe comes back in", () => {
        const { track, live } = stageOfScreens();
        const last = order().at(-1)!;
        act(() => cmd.selectWindowId(last));
        act(() => void vi.advanceTimersByTime(PAN_MS * 2));
        const index = order().indexOf(last);

        act(() => setFingersDown(true));
        swipe(live, 900);
        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(last);
        expect(track).toHaveClass("returning");
        expect(settleTime(track)).toBe(RETURN_MS);
        expect(panOf(track)).toBe(slidLeft(index));
    });

    /* A close that carries on the way the hand went picks up where the hand left
       off, so it keeps the pace of the ground it has left rather than turning round. */
    it("carries a thrown swipe on without turning it round", () => {
        const { track, live, index } = stageOfScreens();

        act(() => setFingersDown(true));
        swipe(live, 300);
        act(() => setFingersDown(false));

        expect(activeWindow()).toBe(order()[index + 1]);
        expect(track).toHaveClass("sliding");
        expect(track).not.toHaveClass("returning");
        // A throw of 0.3 of a screen leaves the other 0.7 of it to cover.
        expect(settleTime(track)).toBe(settleMs(0.7));
        expect(panOf(track)).toBe(slidLeft(index + 1));
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

    it("leaves a gesture that is mostly vertical to whatever is under it", () => {
        const { track, live } = stageOfScreens();

        expect(swipe(live, 60, 50)).toBe(false);
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(track).not.toHaveClass("panning");
    });

    /*
     * A switch from the keyboard mid-swipe takes the session off the screens the
     * gesture was dragging between, so the gesture has nothing left to hold and
     * its snap must not pull the session back.
     */
    it("lets go when the session is switched out from under it", () => {
        const { live, index } = stageOfScreens();
        const chosen = order()[index + 4];

        swipe(live, 100);
        act(() => cmd.selectWindowId(chosen));
        act(() => void vi.advanceTimersByTime(SPENT_END_MS));

        expect(activeWindow()).toBe(chosen);
    });

    /*
     * A screen closing elsewhere moves the session along the track without
     * switching it, so the swipe is no longer counting from the screen it
     * started on and no switch plans a slide. The track still has to come back:
     * left where the finger had it, the whole session sits half a screen off the
     * stage with a band of shell down the edge, and nothing ever puts it back.
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

        expect(activeWindow()).toBe(on);
        expect(track).not.toHaveClass("panning");
        expect(panOf(track)).toBe(slidLeft(order().indexOf(on)));
    });

    /*
     * Dragging is direct manipulation rather than animation, so it still follows
     * the finger with motion reduced; only the close at the end stops being a slide.
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
        expect(panOf(track)).toBe(slidLeft(index + 1));
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
