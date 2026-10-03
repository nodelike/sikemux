import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport } from "./transport";

export interface SimulatorScreen {
    width: number;
    height: number;
}

/** The simulator an agent attached, as `sim_attach` reports it. */
export interface SimulatorAttached {
    agentId: string;
    udid: string;
    name: string;
    os: string;
    screen: SimulatorScreen | null;
}

/** The device Xcode draws around a simulator's screen: its size and where the screen sits in it, in points. */
export interface SimulatorChrome {
    width: number;
    height: number;
    screen: { x: number; y: number; width: number; height: number };
}

/** Which way the device is turned; landscape screens are as wide as they are tall upright. */
export type Orientation = "portrait" | "portraitUpsideDown" | "landscapeLeft" | "landscapeRight";

/** What Settings shows about the simulator. */
export interface SimulatorSetup {
    xcode: string | null;
    runtimes: string[];
    helper: string;
}

/** A simulator Xcode has, for the person to pick from. */
export interface SimulatorDevice {
    udid: string;
    name: string;
    os: string;
    booted: boolean;
    screen: SimulatorScreen | null;
}

/** A new frame of a simulator's screen is ready at `frameUrl`, or its stream ended with `error`. */
export interface SimulatorFrame {
    udid: string;
    frame?: number;
    error?: string;
}

/** What the person does on the screen, in device points. */
export type SimulatorInput =
    | { type: "touch"; phase: "down" | "move" | "up"; x: number; y: number }
    | { type: "button"; button: "home" | "lock" }
    | { type: "type"; text: string };

export const frameUrl = (udid: string, frame: number): string => `sim://localhost/${encodeURIComponent(udid)}/${frame}`;
export const chromeUrl = (udid: string, part: "chrome" | "mask"): string => `sim://localhost/${encodeURIComponent(udid)}/${part}`;

export const simulatorApi = {
    /** Starts the live view, and answers with the device to draw around it when Xcode has one. */
    openView: (udid: string) => invoke<SimulatorChrome | null>("simulator_view_open", { udid }),
    closeView: (udid: string) => invoke<void>("simulator_view_close", { udid }),
    input: (udid: string, input: SimulatorInput) => invoke<void>("simulator_input", { udid, input }),
    devices: () => invoke<SimulatorDevice[]>("simulator_devices"),
    available: () => invoke<boolean>("simulator_available"),
    setEnabled: (enabled: boolean) => invoke<void>("simulator_set_enabled", { enabled }),
    setup: () => invoke<SimulatorSetup>("simulator_setup"),
    /** The device to show when the person opens the simulator for this agent. */
    preferred: (agentId: string) => invoke<SimulatorDevice>("simulator_preferred", { agentId }),
    /** Boots the device and makes it the agent's, so both look at the same screen. */
    attach: (agentId: string, udid: string) => invoke<SimulatorDevice>("simulator_attach", { agentId, udid }),
    shutdown: (udid: string) => invoke<void>("simulator_shutdown", { udid }),
    rotate: (udid: string, orientation: Orientation) => invoke<void>("simulator_rotate", { udid, orientation }),
    orientation: (udid: string) => invoke<Orientation>("simulator_orientation", { udid }),
    subscribeRotated: (listener: (rotated: { udid: string; orientation: Orientation }) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<{ udid: string; orientation: Orientation }>("simulator-rotated", (event) => listener(event.payload), {
            signal,
        }),
    /** Saves the screen to the Desktop as `name`, and answers with where it went. */
    saveScreenshot: (udid: string, name: string) => invoke<string>("simulator_save_screenshot", { udid, name }),
    subscribeFrames: (listener: (frame: SimulatorFrame) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<SimulatorFrame>("simulator-frame", (event) => listener(event.payload), { signal }),
    subscribeDetached: (listener: (detached: { agentId: string; udid: string }) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<{ agentId: string; udid: string }>("simulator-detached", (event) => listener(event.payload), { signal }),
    subscribeAttached: (listener: (attached: SimulatorAttached) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<SimulatorAttached>("simulator-attached", (event) => listener(event.payload), { signal }),
};
