import { act, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { simulatorApi, type Orientation, type SimulatorFrame } from "../api/simulator";
import * as cmd from "../state/commands";
import { devicePickerOptions, fitInside, screenshotName, SimulatorView, toDevicePoint, typedText, uprightTurn } from "./SimulatorView";

vi.mock("../api/simulator", async () => {
    const actual = await vi.importActual<typeof import("../api/simulator")>("../api/simulator");
    return {
        ...actual,
        simulatorApi: {
            openView: vi.fn(),
            closeView: vi.fn(),
            input: vi.fn(),
            devices: vi.fn(),
            attach: vi.fn(),
            shutdown: vi.fn(),
            saveScreenshot: vi.fn(),
            subscribeFrames: vi.fn(),
            subscribeAttached: vi.fn(),
            subscribeRotated: vi.fn(),
            rotate: vi.fn(),
            orientation: vi.fn(),
        },
    };
});

afterEach(() => vi.clearAllMocks());

const iPhone = { width: 402, height: 874 };

describe("mapping a click to the simulator's screen", () => {
    it("scales a point on a screen drawn at half size to device points", () => {
        const drawn = { left: 100, top: 50, width: 201, height: 437 };
        expect(toDevicePoint(drawn, iPhone, 100 + 170, 50 + 217)).toEqual({ x: 340, y: 434 });
    });

    it("leaves out the margins when the box is wider than the screen", () => {
        const wide = { left: 0, top: 0, width: 1000, height: 874 };
        expect(toDevicePoint(wide, iPhone, 299, 400)).toEqual({ x: 0, y: 400 });
        expect(toDevicePoint(wide, iPhone, 200, 400)).toBeNull();
        expect(toDevicePoint(wide, iPhone, 800, 400)).toBeNull();
    });

    it("misses when there is nothing to draw on", () => {
        expect(toDevicePoint({ left: 0, top: 0, width: 0, height: 0 }, iPhone, 0, 0)).toBeNull();
    });
});

describe("a turned simulator", () => {
    it("turns the picture back so the app reads upright", () => {
        expect(uprightTurn("portrait")).toBe(0);
        expect(uprightTurn("landscapeLeft")).toBe(-90);
        expect(uprightTurn("landscapeRight")).toBe(90);
    });
});

describe("keys typed into the simulator", () => {
    const key = (key: string, modifiers: Partial<Record<"metaKey" | "ctrlKey" | "altKey", boolean>> = {}) =>
        typedText({ key, metaKey: false, ctrlKey: false, altKey: false, ...modifiers });

    it("types characters, Return, Tab and Backspace", () => {
        expect(key("a")).toBe("a");
        expect(key("A")).toBe("A");
        expect(key("Enter")).toBe("\n");
        expect(key("Tab")).toBe("\t");
        expect(key("Backspace")).toBe("\b");
    });

    it("leaves shortcuts and named keys to Sikemux", () => {
        expect(key("w", { metaKey: true })).toBeNull();
        expect(key("c", { ctrlKey: true })).toBeNull();
        expect(key("ArrowLeft")).toBeNull();
        expect(key("Shift")).toBeNull();
    });
});

describe("the simulator view", () => {
    const device = { udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", screen: iPhone };

    function mount(live: boolean) {
        let deliver: (frame: SimulatorFrame) => void = () => {};
        let turn: (rotated: { udid: string; orientation: Orientation }) => void = () => {};
        vi.mocked(simulatorApi.subscribeRotated).mockImplementation(async (listener) => {
            turn = listener;
            return () => {};
        });
        vi.mocked(simulatorApi.orientation).mockResolvedValue("portrait");
        vi.mocked(simulatorApi.rotate).mockResolvedValue();
        vi.mocked(simulatorApi.subscribeFrames).mockImplementation(async (listener) => {
            deliver = listener;
            return () => {};
        });
        vi.mocked(simulatorApi.openView).mockResolvedValue(null);
        vi.mocked(simulatorApi.attach).mockResolvedValue({ ...device, booted: true });
        vi.mocked(simulatorApi.closeView).mockResolvedValue();
        vi.mocked(simulatorApi.input).mockResolvedValue();
        vi.mocked(simulatorApi.devices).mockResolvedValue([
            { udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", booted: true, screen: iPhone },
            { udid: "U2", name: "iPhone 17", os: "iOS 26.0", booted: false, screen: { width: 402, height: 874 } },
        ]);
        const view = render(<SimulatorView agentId="agent-1" simulator={device} live={live} />);
        return {
            view,
            screen: within(view.container),
            deliver: (frame: SimulatorFrame) => act(() => deliver(frame)),
            turn: (orientation: Orientation, udid = "U1") => act(() => turn({ udid, orientation })),
        };
    }

    it("starts the device for the agent, then watches the screen only while it is live", async () => {
        const { view, screen, deliver } = mount(true);
        expect(screen.getByText("Starting iPhone 18 Pro…")).toBeTruthy();
        expect(simulatorApi.attach).toHaveBeenCalledWith("agent-1", "U1");
        await vi.waitFor(() => expect(simulatorApi.openView).toHaveBeenCalledWith("U1"));
        expect(screen.getByText("Connecting to iPhone 18 Pro…")).toBeTruthy();

        deliver({ udid: "other", frame: 1 });
        expect(screen.queryByRole("img")).toBeNull();
        deliver({ udid: "U1", frame: 3 });
        expect(screen.getByRole("img").getAttribute("src")).toBe("sim://localhost/U1/3");

        view.rerender(<SimulatorView agentId="agent-1" simulator={device} live={false} />);
        expect(simulatorApi.closeView).toHaveBeenCalledWith("U1");
    });

    it("says why the stream stopped", async () => {
        const { screen, deliver } = mount(true);
        await vi.waitFor(() => expect(simulatorApi.subscribeFrames).toHaveBeenCalled());
        deliver({ udid: "U1", error: "the simulator stream answered HTTP/1.1 404" });
        expect(screen.getByText(/The simulator view stopped: the simulator stream answered/)).toBeTruthy();
    });

    async function onScreen() {
        const mounted = mount(true);
        await vi.waitFor(() => expect(simulatorApi.subscribeFrames).toHaveBeenCalled());
        mounted.deliver({ udid: "U1", frame: 1 });
        mounted.screen.getByRole("img").getBoundingClientRect = () => ({ left: 0, top: 0, width: 201, height: 437 }) as DOMRect;
        const stage = mounted.screen.getByRole("application");
        stage.setPointerCapture = () => {};
        return { ...mounted, stage };
    }

    const sent = () => vi.mocked(simulatorApi.input).mock.calls.map(([, input]) => input);

    it("turns the device, and maps a click on the turned screen to the app's own points", async () => {
        const { screen, stage, turn } = await onScreen();
        fireEvent.click(screen.getByRole("button", { name: "Rotate" }));
        expect(simulatorApi.rotate).toHaveBeenCalledWith("U1", "landscapeLeft");

        turn("landscapeRight", "other");
        turn("landscapeLeft");
        const phone = screen.getByRole("img").closest(".simulator-phone") as HTMLElement;
        expect(phone.style.transform).toContain("rotate(-90deg)");
        screen.getByRole("img").getBoundingClientRect = () => ({ left: 0, top: 0, width: 437, height: 201 }) as DOMRect;

        fireEvent.pointerDown(stage, { clientX: 437, clientY: 0, button: 0, pointerId: 1 });
        fireEvent.pointerUp(stage, { clientX: 437, clientY: 0, pointerId: 1 });
        await vi.waitFor(() => expect(sent()[0]).toMatchObject({ type: "touch", x: 874, y: 0 }));

        fireEvent.click(screen.getByRole("button", { name: "Rotate" }));
        expect(simulatorApi.rotate).toHaveBeenLastCalledWith("U1", "landscapeRight");
    });

    it("puts a finger down and lifts it where the screen is clicked, and presses Home", async () => {
        const { screen, stage } = await onScreen();

        fireEvent.pointerDown(stage, { clientX: 170, clientY: 217, button: 0, pointerId: 1 });
        fireEvent.pointerUp(stage, { clientX: 170, clientY: 217, pointerId: 1 });
        fireEvent.click(screen.getByRole("button", { name: "Home" }));

        await vi.waitFor(() =>
            expect(sent()).toEqual([
                { type: "touch", phase: "down", x: 340, y: 434 },
                { type: "touch", phase: "up", x: 340, y: 434 },
                { type: "button", button: "home" },
            ]),
        );
    });

    it("follows a drag live, folding moves that arrive while one is on its way into the latest", async () => {
        const { stage } = await onScreen();

        fireEvent.pointerDown(stage, { clientX: 100, clientY: 400, button: 0, pointerId: 1 });
        fireEvent.pointerMove(stage, { clientX: 100, clientY: 350, pointerId: 1 });
        fireEvent.pointerMove(stage, { clientX: 100, clientY: 300, pointerId: 1 });
        fireEvent.pointerMove(stage, { clientX: 100, clientY: 250, pointerId: 1 });
        fireEvent.pointerUp(stage, { clientX: 100, clientY: 200, pointerId: 1 });

        await vi.waitFor(() =>
            expect(sent()).toEqual([
                { type: "touch", phase: "down", x: 200, y: 800 },
                { type: "touch", phase: "move", x: 200, y: 500 },
                { type: "touch", phase: "up", x: 200, y: 400 },
            ]),
        );
    });

    it("keeps a finger dragged off the phone on its edge, and ignores a press beside it", async () => {
        const { stage } = await onScreen();

        fireEvent.pointerDown(stage, { clientX: 400, clientY: 200, button: 0, pointerId: 1 });
        fireEvent.pointerDown(stage, { clientX: 20, clientY: 200, button: 0, pointerId: 2 });
        fireEvent.pointerUp(stage, { clientX: -50, clientY: 200, pointerId: 2 });

        await vi.waitFor(() =>
            expect(sent()).toEqual([
                { type: "touch", phase: "down", x: 40, y: 400 },
                { type: "touch", phase: "up", x: 0, y: 400 },
            ]),
        );
    });

    it("does not watch a simulator that is not on screen", () => {
        mount(false);
        expect(simulatorApi.openView).not.toHaveBeenCalled();
    });
});

describe("picking a simulator", () => {
    const device = (udid: string, name: string, os: string, booted = false) => ({ udid, name, os, booted, screen: iPhone });

    it("lists the newest iOS first, then by name, and marks the running ones", () => {
        const options = devicePickerOptions([
            device("a", "iPhone 17", "iOS 26.0"),
            device("b", "iPhone 18 Pro", "iOS 27.0", true),
            device("c", "iPad Air 11-inch (M4)", "iOS 27.0"),
            device("d", "iPhone 16e", "iOS 26.0"),
        ]);
        expect(options.map((option) => [option.label, option.detail])).toEqual([
            ["iPad Air 11-inch (M4)", "iOS 27.0"],
            ["iPhone 18 Pro", "iOS 27.0 · running"],
            ["iPhone 16e", "iOS 26.0"],
            ["iPhone 17", "iOS 26.0"],
        ]);
    });

    it("boots the picked device for the agent and shows it in the same tab", async () => {
        const switched = vi.spyOn(cmd, "switchDeskSimulator").mockImplementation(() => {});
        let finish: (device: { udid: string; name: string; os: string; booted: boolean; screen: typeof iPhone }) => void = () => {};
        vi.mocked(simulatorApi.attach)
            .mockResolvedValueOnce({ ...device("U1", "iPhone 18 Pro", "iOS 27.0"), booted: true })
            .mockImplementation(() => new Promise((resolve) => (finish = resolve)));
        vi.mocked(simulatorApi.subscribeFrames).mockResolvedValue(() => {});
        vi.mocked(simulatorApi.openView).mockResolvedValue(null);
        vi.mocked(simulatorApi.devices).mockResolvedValue([device("U1", "iPhone 18 Pro", "iOS 27.0", true), device("U2", "iPhone 17", "iOS 26.0")]);
        const view = render(<SimulatorView agentId="agent-1" simulator={{ ...device("U1", "iPhone 18 Pro", "iOS 27.0") }} live />);
        const screen = within(view.container);
        await vi.waitFor(() => expect(simulatorApi.devices).toHaveBeenCalled());

        fireEvent.click(screen.getByRole("button", { name: /Simulator: iPhone 18 Pro/ }));
        fireEvent.click(await within(document.body).findByRole("option", { name: /iPhone 17/ }));

        expect(simulatorApi.attach).toHaveBeenCalledWith("agent-1", "U2");
        expect(screen.getByText("Starting iPhone 17…")).toBeTruthy();
        await act(async () => finish({ ...device("U2", "iPhone 17", "iOS 26.0"), booted: true }));
        expect(switched).toHaveBeenCalledWith("agent-1", "U1", { udid: "U2", name: "iPhone 17", os: "iOS 26.0", screen: iPhone });
    });

    it("shuts the device down and closes its tab", async () => {
        const closed = vi.spyOn(cmd, "closeDeskItem").mockImplementation(() => {});
        vi.mocked(simulatorApi.shutdown).mockResolvedValue();
        vi.mocked(simulatorApi.devices).mockResolvedValue([]);
        const simulator = { udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", screen: iPhone };
        const view = render(<SimulatorView agentId="agent-1" simulator={simulator} live={false} />);

        fireEvent.click(within(view.container).getByRole("button", { name: "Shut down" }));

        await vi.waitFor(() => expect(closed).toHaveBeenCalledWith("agent-1", { key: "simulator:U1", kind: "simulator", simulator }));
        expect(simulatorApi.shutdown).toHaveBeenCalledWith("U1");
    });
});

describe("the device around the screen", () => {
    const iPhone18 = { width: 454, height: 908, screen: { x: 26, y: 17, width: 402, height: 874 } };

    it("scales the device to the room it has, keeping its proportions", () => {
        expect(fitInside({ width: 1000, height: 454 }, { width: 454, height: 908 })).toEqual({ width: 227, height: 454 });
        expect(fitInside({ width: 227, height: 2000 }, { width: 454, height: 908 })).toEqual({ width: 227, height: 454 });
        expect(fitInside({ width: 0, height: 0 }, { width: 454, height: 908 })).toEqual({ width: 0, height: 0 });
    });

    async function drawn(chrome: typeof iPhone18 | null) {
        let deliver: (frame: SimulatorFrame) => void = () => {};
        vi.mocked(simulatorApi.subscribeFrames).mockImplementation(async (listener) => {
            deliver = listener;
            return () => {};
        });
        vi.mocked(simulatorApi.openView).mockResolvedValue(chrome);
        vi.mocked(simulatorApi.attach).mockResolvedValue({ udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", booted: true, screen: iPhone });
        vi.mocked(simulatorApi.devices).mockResolvedValue([]);
        const view = render(
            <SimulatorView agentId="agent-1" simulator={{ udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", screen: iPhone }} live />,
        );
        await vi.waitFor(() => expect(simulatorApi.openView).toHaveBeenCalled());
        await act(async () => deliver({ udid: "U1", frame: 1 }));
        return view.container;
    }

    it("draws the screen inside Xcode's device, shaped like the real display", async () => {
        const container = await drawn(iPhone18);
        const bezel = container.querySelector<HTMLImageElement>(".simulator-bezel")!;
        const screen = container.querySelector<HTMLImageElement>(".simulator-screen")!;
        expect(bezel.getAttribute("src")).toBe("sim://localhost/U1/chrome");
        expect(screen.style.left).toBe(`${(26 / 454) * 100}%`);
        expect(screen.style.top).toBe(`${(17 / 908) * 100}%`);
        expect(screen.style.width).toBe(`${(402 / 454) * 100}%`);
        expect(screen.style.maskImage).toContain("sim://localhost/U1/mask");
    });

    it("shows the screen bare when Xcode has no drawing of the device", async () => {
        const container = await drawn(null);
        expect(container.querySelector(".simulator-bezel")).toBeNull();
        const screen = container.querySelector<HTMLImageElement>(".simulator-screen")!;
        expect([screen.style.left, screen.style.width, screen.style.height]).toEqual(["0%", "100%", "100%"]);
        expect(screen.style.maskImage).toBe("");
    });
});

describe("screenshots", () => {
    it("are named the way Simulator.app names them", () => {
        expect(screenshotName("iPhone 18 Pro", new Date(2026, 9, 3, 9, 5, 7))).toBe(
            "Simulator Screenshot - iPhone 18 Pro - 2026-10-03 at 09.05.07.png",
        );
        expect(screenshotName("a/b", new Date(2026, 0, 1, 0, 0, 0))).toBe("Simulator Screenshot - a-b - 2026-01-01 at 00.00.00.png");
    });

    it("are saved to the Desktop from the button under the device", async () => {
        vi.mocked(simulatorApi.saveScreenshot).mockResolvedValue("/Users/me/Desktop/Simulator Screenshot - iPhone 18 Pro.png");
        vi.mocked(simulatorApi.devices).mockResolvedValue([]);
        const view = render(
            <SimulatorView agentId="agent-1" simulator={{ udid: "U1", name: "iPhone 18 Pro", os: "iOS 27.0", screen: iPhone }} live={false} />,
        );

        fireEvent.click(within(view.container).getByRole("button", { name: "Screenshot" }));

        await vi.waitFor(() => expect(simulatorApi.saveScreenshot).toHaveBeenCalled());
        const [udid, name] = vi.mocked(simulatorApi.saveScreenshot).mock.calls[0];
        expect(udid).toBe("U1");
        expect(name).toMatch(/^Simulator Screenshot - iPhone 18 Pro - \d{4}-\d{2}-\d{2} at \d{2}\.\d{2}\.\d{2}\.png$/);
    });
});
