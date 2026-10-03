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
    /** In points, which touches are measured in. */
    width: number;
    height: number;
    scale: number;
}

export type SimStreamFormat = "h264" | "mjpeg";

export type SimButton = "home" | "lock" | "side" | "siri" | "volumeUp" | "volumeDown";
export type SimOrientation = "portrait" | "portraitUpsideDown" | "landscapeLeft" | "landscapeRight";
export type SimTouchPhase = "down" | "move" | "up";

export type SimEvent = { type: "progress"; fraction: number };

type Request = { type: string; udid?: string } & Record<string, unknown>;

const call = <T>(request: Request) => invoke<T>("sim_call", { request });

export const simApi = {
    status: () => invoke<SimStatus>("sim_status"),
    prepare: () => invoke<void>("sim_prepare"),
    devices: () => call<{ devices: SimDevice[] }>({ type: "devices" }).then((answer) => answer.devices),
    boot: (udid: string) => call<void>({ type: "boot", udid }),
    shutdown: (udid: string) => call<void>({ type: "shutdown", udid }),
    screen: (udid: string) => call<SimScreen>({ type: "screen", udid }),
    stopStream: (udid: string, format?: SimStreamFormat) => call<void>({ type: "stopStream", udid, format }),
    touch: (udid: string, phase: SimTouchPhase, x: number, y: number) => call<void>({ type: "touch", udid, phase, x, y }),
    text: (udid: string, text: string) => call<void>({ type: "text", udid, text }),
    key: (udid: string, key: string) => call<void>({ type: "key", udid, key }),
    button: (udid: string, button: SimButton) => call<void>({ type: "button", udid, button }),
    orientation: (udid: string, orientation: SimOrientation) => call<void>({ type: "orientation", udid, orientation }),
    screenshot: (udid: string, path: string) => call<{ path: string }>({ type: "screenshot", udid, path }),
    /** Streams the screen through the app; resolves to the id `unwatch` takes. */
    watch: (udid: string, format: SimStreamFormat, onFrame: (frame: ArrayBuffer) => void) => {
        const channel = new Channel<ArrayBuffer>();
        channel.onmessage = onFrame;
        return invoke<number>("sim_watch", { udid, format, onFrame: channel });
    },
    unwatch: (id: number) => invoke<void>("sim_unwatch", { id }),
    subscribe: (listener: (event: SimEvent) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<SimEvent>("sim", (event) => listener(event.payload), { signal }),
};
