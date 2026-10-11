import { Channel } from "@tauri-apps/api/core";
import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";

export interface SimStatus {
    supported: boolean;
    installed: boolean;
    reason: string | null;
}

export interface SimDevice {
    udid: string;
    name: string;
    state: "booted" | "shutdown" | "busy";
    runtime: string;
    model: string;
}

export interface SimScreen {
    /** In points, which touches are measured in, for the screen as it is turned. */
    width: number;
    height: number;
    scale: number;
    orientation: SimOrientation | "faceUp" | "faceDown" | "unknown";
}

export type SimStreamFormat = "h264" | "mjpeg";

export type SimButton = "home" | "lock" | "side" | "siri" | "volumeUp" | "volumeDown" | "action";

/** A side button on the device's frame, in points from the frame's top left. */
export interface SimChromeButton {
    name: string;
    title: string;
    anchor: "left" | "right";
    /** Its left edge from the frame's side, or its right edge when anchored right; `hoverX` is where it slides to. */
    x: number;
    y: number;
    hoverX: number;
    width: number;
    height: number;
    image: string;
    imageDown: string | null;
}

/** The device's frame from Xcode's DeviceKit chrome: the bezel image, the room its side buttons need, and the buttons. */
export interface SimChrome {
    image: string;
    width: number;
    height: number;
    padding: { top: number; left: number; bottom: number; right: number };
    buttons: SimChromeButton[];
}
export type SimOrientation = "portrait" | "portraitUpsideDown" | "landscapeLeft" | "landscapeRight";
export type SimTouchPhase = "down" | "move" | "up";

export type SimEvent = { type: "progress"; fraction: number };

/** The device an agent attached with `sim_attach`, which its desk then shows. */
export interface SimAttached {
    agentId: string;
    udid: string;
    name: string;
}

/** An agent's device, with the project the agent works in. */
export interface SimAttachment extends SimAttached {
    project: string;
}

/** An agent let go of its device: it detached, stopped, or moved to another. */
export interface SimDetached {
    agentId: string;
}

/** `acting` is true while one of the agent's calls drives its device. */
export interface SimActing {
    agentId: string;
    acting: boolean;
}

/** A screen stream ended without `unwatch`, because the helper's stream stopped. */
export interface SimWatchEnded {
    id: number;
    reason: string;
}

/** Frames are reported read in batches this size, so the stream keeps sending. */
const READ_BATCH = 4;
const watchEnds = new Map<number, AbortController>();

/** What Settings shows about the simulator. */
export interface SimSetup {
    xcode: string | null;
    runtimes: string[];
    helper: string;
}

type Request = { type: string; udid?: string } & Record<string, unknown>;

const call = <T>(request: Request) => invoke<T>("sim_call", { request });

export const simApi = {
    status: () => invoke<SimStatus>("sim_status"),
    prepare: () => invoke<void>("sim_prepare"),
    devices: () => call<{ devices: SimDevice[] }>({ type: "devices" }).then((answer) => answer.devices),
    boot: (udid: string) => call<void>({ type: "boot", udid }),
    shutdown: (udid: string) => call<void>({ type: "shutdown", udid }),
    screen: (udid: string) => call<SimScreen>({ type: "screen", udid }),
    /** The screen's outline, upright and at full resolution, as a PNG data URL; null for a square screen. */
    mask: (udid: string) => call<{ mask: string | null }>({ type: "mask", udid }).then((answer) => answer.mask),
    chrome: (udid: string) => call<{ chrome: SimChrome | null }>({ type: "chrome", udid }).then((answer) => answer.chrome),
    touch: (udid: string, phase: SimTouchPhase, x: number, y: number) => call<void>({ type: "touch", udid, phase, x, y }),
    text: (udid: string, text: string) => call<void>({ type: "text", udid, text }),
    key: (udid: string, key: string) => call<void>({ type: "key", udid, key }),
    button: (udid: string, button: SimButton, phase?: "down" | "up") => call<void>({ type: "button", udid, button, phase }),
    /** Turns the device, or without an orientation reads which way it is turned; resolves to the orientation. */
    orientation: (udid: string, orientation?: SimOrientation) => invoke<SimOrientation>("simulator_orientation", { udid, orientation }),
    /** Drags a finger from one point to another, in device points. */
    swipe: (udid: string, from: { x: number; y: number }, to: { x: number; y: number }, durationMs?: number) =>
        call<void>({ type: "swipe", udid, x: from.x, y: from.y, toX: to.x, toY: to.y, duration: durationMs == null ? undefined : durationMs / 1000 }),
    screenshot: (udid: string, path: string) => call<{ path: string }>({ type: "screenshot", udid, path }),
    /**
     * Streams the screen through the app; resolves to the id `unwatch` takes.
     * `onEnd` hears why a stream stopped on its own; start a new watch to carry on.
     */
    watch: async (udid: string, format: SimStreamFormat, onFrame: (frame: ArrayBuffer) => void, onEnd?: (reason: string) => void) => {
        const channel = new Channel<ArrayBuffer>();
        let id: number | null = null;
        let frames = 0;
        const reportRead = () => {
            if (id != null) void invoke<void>("sim_watch_read", { id, frames }).catch(() => {});
        };
        channel.onmessage = (frame) => {
            onFrame(frame);
            frames += 1;
            if (frames % READ_BATCH === 0) reportRead();
        };
        const ends = new AbortController();
        const ended: SimWatchEnded[] = [];
        await getIpcTransport().subscribe<SimWatchEnded>(
            "simulator-watch-ended",
            (event) => {
                if (id == null) ended.push(event.payload);
                else if (event.payload.id === id) finish(event.payload.reason);
            },
            { signal: ends.signal },
        );
        const finish = (reason: string) => {
            ends.abort();
            if (id != null) watchEnds.delete(id);
            onEnd?.(reason);
        };
        try {
            id = await invoke<number>("sim_watch", { udid, format, onFrame: channel });
        } catch (error) {
            ends.abort();
            throw error;
        }
        watchEnds.set(id, ends);
        reportRead();
        const early = ended.find((end) => end.id === id);
        if (early) finish(early.reason);
        return id;
    },
    unwatch: (id: number) => {
        watchEnds.get(id)?.abort();
        watchEnds.delete(id);
        return invoke<void>("sim_unwatch", { id });
    },
    subscribe: (listener: (event: SimEvent) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<SimEvent>("sim", (event) => listener(event.payload), { signal }),
    setup: () => invoke<SimSetup>("simulator_setup"),
    /** Whether agents started from now on get the `sim_*` tools. */
    offerToAgents: (enabled: boolean) => invoke<void>("simulator_set_enabled", { enabled }),
    subscribeAttached: (listener: (attached: SimAttached) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<SimAttached>("simulator-attached", (event) => listener(event.payload), { signal }),
    subscribeDetached: (listener: (detached: SimDetached) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<SimDetached>("simulator-detached", (event) => listener(event.payload), { signal }),
    subscribeActing: (listener: (acting: SimActing) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<SimActing>("simulator-acting", (event) => listener(event.payload), { signal }),
    /** Every agent's device, for "in use by" in the device picker. */
    attachments: () => invoke<SimAttachment[]>("simulator_attachments"),
    /**
     * The person picked `udid` on an agent's desk: an attached agent moves to it
     * (a `simulator-attached` event follows), and one not yet attached gets it on `sim_attach`.
     */
    setDeskDevice: (agentId: string, udid: string) => invoke<void>("simulator_set_desk_device", { agentId, udid }),
};
