import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { simApi } from "../api/sim";
import { noteSimulatorActing, noteSimulatorAttached, noteSimulatorDetached } from "../state/simulatorAgents";
import { playScreen } from "./screenStream";
import { deviceOptions, devicePoint, frameLayout, keyForDevice, screenshotPath, scrollStep, SimulatorPane } from "./SimulatorPane";

vi.mock("../api/sim", () => ({
    simApi: {
        status: vi.fn(),
        prepare: vi.fn(),
        subscribe: vi.fn(),
        devices: vi.fn(),
        screen: vi.fn(),
        mask: vi.fn(() => Promise.resolve(null)),
        chrome: vi.fn(() => Promise.resolve(null)),
        orientation: vi.fn(() => Promise.resolve("landscapeRight")),
        touch: vi.fn(),
        text: vi.fn(),
        key: vi.fn(),
        boot: vi.fn(),
        shutdown: vi.fn(),
        subscribeAttached: vi.fn(),
        setDeskDevice: vi.fn(),
    },
}));

vi.mock("./screenStream", () => ({ playScreen: vi.fn() }));

const deviceScreen = { width: 402, height: 874, scale: 3, orientation: "portrait" as const };
/* A 1206 x 2622 frame drawn into a 600 x 600 box: scaled to fit its height and centred, with bars either side. */
const canvas = { width: 1206, height: 2622, rect: { left: 100, top: 50, width: 600, height: 600 } };
const drawnLeft = 100 + (600 - 1206 * (600 / 2622)) / 2;

describe("pointing at the simulator's screen", () => {
    it("finds the device point under the pointer, past the bars a contained canvas leaves", () => {
        expect(devicePoint(canvas, deviceScreen, drawnLeft, 50)).toEqual({ x: 0, y: 0 });
        const centre = devicePoint(canvas, deviceScreen, 400, 350)!;
        expect(centre.x).toBeCloseTo(201);
        expect(centre.y).toBeCloseTo(437);
    });

    it("ignores the bars beside the screen and a canvas with nothing drawn yet", () => {
        expect(devicePoint(canvas, deviceScreen, 105, 300)).toBeNull();
        expect(devicePoint({ ...canvas, width: 0, height: 0 }, deviceScreen, 400, 350)).toBeNull();
    });

    it("holds a dragged finger at the screen's edge", () => {
        expect(devicePoint(canvas, deviceScreen, 105, 700, { clamp: true })).toEqual({ x: 0, y: 874 });
    });

    it("reads a turned device's points straight off its canvas, which is drawn turned", () => {
        const landscape = { width: 874, height: 402, scale: 3, orientation: "landscapeLeft" as const };
        const turned = { width: 2622, height: 1206, rect: canvas.rect };
        const drawnTop = 50 + (600 - 1206 * (600 / 2622)) / 2;
        expect(devicePoint(turned, landscape, 100, drawnTop)).toEqual({ x: 0, y: 0 });
        const centre = devicePoint(turned, landscape, 400, 350)!;
        expect(centre.x).toBeCloseTo(437);
        expect(centre.y).toBeCloseTo(201);
    });

    it("scrolls by moving a finger the other way, and says when it reaches the screen's edge", () => {
        expect(scrollStep({ x: 201, y: 437 }, deviceScreen, 1, 0, 100)).toEqual({ next: { x: 201, y: 337 }, stuck: false });
        expect(scrollStep({ x: 201, y: 437 }, deviceScreen, 1, 0, 10_000)).toEqual({ next: { x: 201, y: 0 }, stuck: true });
    });
});

describe("typing into the device", () => {
    it("sends what Option types, and leaves Command and Tab to the app", () => {
        expect(keyForDevice({ key: "å", metaKey: false, ctrlKey: false })).toEqual({ text: "å" });
        expect(keyForDevice({ key: "Enter", metaKey: false, ctrlKey: false })).toEqual({ key: "Enter" });
        expect(keyForDevice({ key: "c", metaKey: true, ctrlKey: false })).toBeNull();
        expect(keyForDevice({ key: "Tab", metaKey: false, ctrlKey: false })).toBeNull();
        expect(keyForDevice({ key: "Dead", metaKey: false, ctrlKey: false })).toBeNull();
    });

    it("names a screenshot with only what a file name can hold", () => {
        expect(screenshotPath("iPhone 17 / Pro: test", new Date("2026-10-08T01:02:03.456Z"))).toBe(
            "~/Desktop/Simulator iPhone 17 - Pro- test 2026-10-08T01-02-03.png",
        );
    });
});

describe("the simulator pane", () => {
    const simulator = { id: "sim-1", udid: "UDID-1", deviceName: "iPhone 17" };
    let firstFrame: () => void = () => {};

    beforeEach(() => {
        vi.mocked(simApi.status).mockResolvedValue({ supported: true, installed: true, reason: null });
        vi.mocked(simApi.devices).mockResolvedValue([
            { udid: "UDID-1", name: "iPhone 17", state: "booted", runtime: "iOS 26.0", model: "iPhone18,1" },
        ]);
        vi.mocked(simApi.screen).mockResolvedValue(deviceScreen);
        vi.mocked(simApi.touch).mockResolvedValue(undefined);
        vi.mocked(simApi.text).mockResolvedValue(undefined);
        vi.mocked(simApi.subscribeAttached).mockResolvedValue(() => {});
        vi.mocked(playScreen).mockImplementation((_udid, target, events) => {
            target.width = 1206;
            target.height = 2622;
            firstFrame = () => events.onFirstFrame?.();
            return { stop: vi.fn(), markInput: vi.fn(), turn: vi.fn(), clip: vi.fn() };
        });
        HTMLCanvasElement.prototype.setPointerCapture = vi.fn();
        HTMLCanvasElement.prototype.getBoundingClientRect = () => ({ ...canvas.rect, right: 700, bottom: 650, x: 100, y: 50, toJSON: () => ({}) });
    });

    afterEach(() => {
        cleanup();
        document.body.replaceChildren();
        noteSimulatorDetached("agent-1");
        vi.clearAllMocks();
    });

    function chrome() {
        const tools = document.body.appendChild(document.createElement("div"));
        const dot = document.body.appendChild(document.createElement("span"));
        return { tools, dot, menu: null, closeMenu: vi.fn() };
    }

    async function showScreen(slots = chrome()) {
        render(<SimulatorPane agentId="agent-1" simulator={simulator} visible chrome={slots} />);
        const surface = await screen.findByLabelText("iPhone 17 screen");
        await waitFor(() => expect(playScreen).toHaveBeenCalled());
        await waitFor(() => expect(simApi.screen).toHaveBeenCalled());
        act(() => firstFrame());
        return surface;
    }

    const pointer = (x: number, y: number, extra: Record<string, unknown> = {}) => ({
        pointerId: 1,
        isPrimary: true,
        button: 0,
        clientX: x,
        clientY: y,
        ...extra,
    });

    it("rotates clockwise, the way its button's arrow points", async () => {
        const slots = chrome();
        await showScreen(slots);

        fireEvent.click(slots.tools.querySelector('[aria-label="Rotate"]')!);

        await waitFor(() => expect(simApi.orientation).toHaveBeenCalledWith("UDID-1", "landscapeRight"));
    });

    it("says it is shutting down while the device winds down, and shows why a shutdown failed", async () => {
        const slots = chrome();
        await showScreen(slots);
        vi.mocked(simApi.devices).mockResolvedValue([{ udid: "UDID-1", name: "iPhone 17", state: "busy", runtime: "iOS 26.0", model: "iPhone18,1" }]);
        vi.mocked(simApi.shutdown).mockRejectedValue({ category: "internal", message: "The device would not shut down." });

        fireEvent.click(slots.tools.querySelector('[aria-label="Shut down"]')!);

        expect(await screen.findByText("Shutting down iPhone 17…")).toBeInTheDocument();
        expect(slots.tools.querySelector('[aria-label="Shutting down…"]')).not.toBeNull();
        expect(screen.getByText("The device would not shut down.")).toBeInTheDocument();
    });

    it("marks its screen as somewhere keys go, so the app's own single-key shortcuts stay out", async () => {
        const surface = await showScreen();
        expect(surface).toHaveAttribute("data-takes-keys");
        fireEvent.keyDown(surface, { key: "å", altKey: true });
        await waitFor(() => expect(simApi.text).toHaveBeenCalledWith("UDID-1", "å"));
    });

    it("sends only the latest of the moves that pile up behind one in flight", async () => {
        let release: () => void = () => {};
        vi.mocked(simApi.touch).mockImplementationOnce(() => new Promise((resolve) => (release = () => resolve(undefined))));
        const surface = await showScreen();

        fireEvent.pointerDown(surface, pointer(400, 350));
        fireEvent.pointerMove(surface, pointer(400, 360));
        fireEvent.pointerMove(surface, pointer(400, 370));
        fireEvent.pointerMove(surface, pointer(400, 380));
        fireEvent.pointerUp(surface, pointer(400, 380));
        await waitFor(() => expect(simApi.touch).toHaveBeenCalledTimes(1));
        await act(async () => release());

        await waitFor(() => expect(vi.mocked(simApi.touch).mock.calls.map((call) => call[1])).toEqual(["down", "move", "up"]));
        expect(vi.mocked(simApi.touch).mock.calls[1][3]).toBeCloseTo(((380 - 50) / 600) * 874);
    });

    it("lifts the finger when the pointer is taken away, and ignores moves with no finger down", async () => {
        const surface = await showScreen();

        fireEvent.pointerMove(surface, pointer(400, 360));
        fireEvent.pointerDown(surface, pointer(400, 350, { button: 2 }));
        fireEvent.pointerDown(surface, pointer(400, 350));
        fireEvent.pointerMove(surface, pointer(900, 360));
        fireEvent.pointerCancel(surface, pointer(900, 360));
        fireEvent.lostPointerCapture(surface, pointer(900, 360));

        await waitFor(() => expect(vi.mocked(simApi.touch).mock.calls.map((call) => call[1])).toEqual(["down", "move", "up"]));
        const [, , upX] = vi.mocked(simApi.touch).mock.calls[2];
        expect(upX).toBe(402);
    });

    it("keeps the person's hands off the device while the agent drives it", async () => {
        const surface = await showScreen();
        act(() => noteSimulatorActing("agent-1", true));

        expect(screen.getByRole("status")).toHaveTextContent("is using the device");
        fireEvent.pointerDown(surface, pointer(400, 350));
        fireEvent.keyDown(surface, { key: "a" });
        expect(simApi.touch).not.toHaveBeenCalled();
        expect(simApi.text).not.toHaveBeenCalled();

        act(() => noteSimulatorActing("agent-1", false));
        expect(screen.queryByRole("status")).not.toBeInTheDocument();
        fireEvent.pointerDown(surface, pointer(400, 350));
        await waitFor(() => expect(simApi.touch).toHaveBeenCalledWith("UDID-1", "down", expect.any(Number), expect.any(Number)));
    });

    it("says when the agent is on another device", async () => {
        await showScreen();
        act(() => noteSimulatorAttached({ agentId: "agent-1", udid: "UDID-2", name: "iPad Air" }));

        expect(screen.getByText(/is on iPad Air/)).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Show it" })).toBeInTheDocument();

        act(() => noteSimulatorDetached("agent-1"));
        expect(screen.queryByText(/is on iPad Air/)).not.toBeInTheDocument();
    });
});

describe("deviceOptions", () => {
    it("lists running devices first, then iPhones, then iPads, and says who holds one", () => {
        const device = (udid: string, name: string, state: "booted" | "shutdown") => ({ udid, name, state, runtime: "iOS 27.0", model: name });
        const options = deviceOptions(
            [device("pad", "iPad (A16)", "shutdown"), device("air", "iPhone Air", "shutdown"), device("pro", "iPhone 18 Pro", "booted")],
            (udid) => (udid === "air" ? "/code/shop" : undefined),
        );

        expect(options.map((option) => [option.label, option.group])).toEqual([
            ["iPhone 18 Pro", "Running"],
            ["iPhone Air", "iPhone"],
            ["iPad (A16)", "iPad"],
        ]);
        expect(options[1]).toMatchObject({ meta: "iOS 27.0", detail: "In use by shop" });
    });
});

describe("frameLayout", () => {
    const art = {
        image: "",
        width: 436,
        height: 908,
        padding: { top: 0, left: 9, bottom: 0, right: 9 },
        buttons: [],
    };
    const upright = { width: 402, height: 874, scale: 3, orientation: "portrait" as const };

    it("fits the frame to the room, with the screen in its opening", () => {
        const layout = frameLayout(art, upright, { width: 1000, height: 454 })!;
        expect(layout.scale).toBeCloseTo(0.5);
        expect(layout.width).toBeCloseTo(227);
        expect(layout.screen).toEqual({ left: (9 + 17) * 0.5, top: 17 * 0.5, width: 201, height: 437 });
        expect(frameLayout(art, upright, { width: 5000, height: 1816 })!.scale).toBeCloseTo(2);
    });

    it("turns the frame with the device and keeps the screen in its opening", () => {
        const layout = frameLayout(art, { width: 874, height: 402, scale: 3, orientation: "landscapeRight" }, { width: 908, height: 454 })!;
        expect(layout.scale).toBe(1);
        expect(layout.frame.degrees).toBe(90);
        expect([layout.width, layout.height]).toEqual([908, 454]);
        expect(layout.screen.left).toBeCloseTo(17);
        expect(layout.screen.top).toBeCloseTo(9 + 17);
        expect([layout.screen.width, layout.screen.height]).toEqual([874, 402]);
    });
});
