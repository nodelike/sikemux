import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";

export interface VoiceStatus {
    supported: boolean;
    installed: boolean;
    reason: string | null;
}

export type VoiceStage = "download" | "compile" | "vocabulary";

export type VoiceEvent =
    | { type: "progress"; stage: VoiceStage; fraction: number }
    | { type: "ready" }
    | { type: "listening" }
    | { type: "partial"; text: string }
    | { type: "transcript"; text: string }
    | { type: "cancelled" }
    | { type: "exited" }
    | { type: "error"; reason: string; message: string };

export const voiceApi = {
    status: () => invoke<VoiceStatus>("voice_status"),
    prepare: () => invoke<void>("voice_prepare"),
    start: (vocabulary: string[]) => invoke<void>("voice_start", { vocabulary }),
    stop: () => invoke<void>("voice_stop"),
    cancel: () => invoke<void>("voice_cancel"),
    shutdown: () => invoke<void>("voice_shutdown"),
    subscribe: (listener: (event: VoiceEvent) => void, signal: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<VoiceEvent>("voice", (event) => listener(event.payload), { signal }),
};
