import { useEffect } from "react";
import { create } from "zustand";
import { voiceApi, type VoiceEvent, type VoiceStage } from "../api/voice";
import { setVoiceDictation } from "../state/commands";
import { useStore } from "../state/store";
import { focusedTextInsertTarget, insertText } from "../state/textInsertRegistry";
import { notify } from "../state/toast";

export type VoicePhase = "off" | "unsupported" | "preparing" | "ready" | "listening" | "transcribing";

export interface VoiceState {
    phase: VoicePhase;
    reason: string | null;
    stage: VoiceStage | null;
    fraction: number;
    /** Where the words being spoken now will be typed. */
    target: HTMLElement | null;
    /** What has been heard so far, while the words are still being spoken. */
    partial: string;
}

export const useVoice = create<VoiceState>(() => ({ phase: "off", reason: null, stage: null, fraction: 0, target: null, partial: "" }));

export const HOLD_KEY = "AltRight";
/* Right Option also starts shortcuts, so the microphone waits to see it held on its own. */
const START_DELAY_MS = 150;

let lifecycle: Promise<void> = Promise.resolve();
let pendingStart: number | null = null;
let recording = false;

const set = (next: Partial<VoiceState>) => useVoice.setState(next);

function inOrder(step: () => Promise<void>): void {
    lifecycle = lifecycle.then(step).catch((error) => set({ phase: "off", reason: String(error) }));
}

async function prepare(): Promise<void> {
    const status = await voiceApi.status();
    if (!status.supported) {
        set({ phase: "unsupported", reason: status.reason });
        return;
    }
    set({ phase: "preparing", reason: null, stage: status.installed ? "compile" : "download", fraction: 0 });
    await voiceApi.prepare();
}

async function shutdown(): Promise<void> {
    abandonHold();
    set({ phase: "off", reason: null, stage: null, fraction: 0 });
    await voiceApi.shutdown();
}

function deliver(text: string): void {
    const { target } = useVoice.getState();
    const destination = target?.isConnected ? target : focusedTextInsertTarget();
    set({ target: null, partial: "" });
    if (!text) return;
    if (destination && insertText(destination, text)) return;
    void navigator.clipboard
        .writeText(text)
        .then(() => notify("info", "No agent or terminal had focus, so the dictation was copied to the clipboard."))
        .catch(() => notify("error", `Dictation had nowhere to go: ${text}`));
}

export function handleVoiceEvent(event: VoiceEvent): void {
    switch (event.type) {
        case "progress":
            set({ phase: "preparing", stage: event.stage, fraction: event.fraction });
            return;
        case "ready":
            set({ phase: "ready", reason: null, stage: null, fraction: 0 });
            return;
        case "listening":
            if (useVoice.getState().phase === "ready") set({ phase: "listening", partial: "" });
            return;
        case "partial": {
            const { phase } = useVoice.getState();
            if (phase === "listening" || phase === "transcribing") set({ partial: event.text });
            return;
        }
        case "transcript":
            set({ phase: "ready" });
            deliver(event.text);
            return;
        case "cancelled":
            set({ phase: "ready", target: null, partial: "" });
            return;
        case "exited":
            recording = false;
            set({ phase: "off", target: null, partial: "", reason: "The voice helper stopped. Hold the key again to restart it." });
            return;
        case "error":
            recording = false;
            if (event.reason === "models") {
                set({ phase: "off", reason: event.message, stage: null, fraction: 0, target: null, partial: "" });
            } else {
                set({ phase: "ready", target: null, partial: "" });
            }
            notify("error", event.message);
            return;
    }
}

function abandonHold(): void {
    if (pendingStart !== null) {
        window.clearTimeout(pendingStart);
        pendingStart = null;
        set({ target: null });
    }
    if (!recording) return;
    recording = false;
    set({ target: null, partial: "" });
    void voiceApi.cancel();
}

function startWhenReady(): void {
    const { phase, stage, fraction } = useVoice.getState();
    if (phase === "ready") {
        recording = true;
        void voiceApi.start();
        return;
    }
    set({ target: null });
    if (phase === "off") {
        notify("info", "Loading the speech model. Try again in a moment.");
        inOrder(prepare);
    } else if (phase === "preparing") {
        const what = stage === "download" ? `downloading (${Math.round(fraction * 100)}%)` : "loading";
        notify("info", `The speech model is still ${what}.`);
    }
}

function beginHold(): void {
    if (pendingStart !== null || recording) return;
    set({ target: focusedTextInsertTarget() });
    pendingStart = window.setTimeout(() => {
        pendingStart = null;
        startWhenReady();
    }, START_DELAY_MS);
}

function finishRecording(): void {
    recording = false;
    set({ phase: "transcribing" });
    void voiceApi.stop();
}

function endHold(): void {
    if (pendingStart !== null) {
        abandonHold();
        return;
    }
    if (recording) finishRecording();
}

/** Click once to start dictating into `into`, and again to type what was said. */
export function toggleDictation(into: HTMLElement): void {
    if (!useStore.getState().voiceDictation) {
        setVoiceDictation(true);
        notify("info", "Dictation is on. The speech model downloads once, about 600 MB, then click the microphone again.");
        return;
    }
    if (recording) {
        finishRecording();
        return;
    }
    if (pendingStart !== null) return;
    set({ target: into });
    startWhenReady();
}

export function onVoiceKeyDown(event: KeyboardEvent): void {
    if (event.code !== HOLD_KEY) {
        abandonHold();
        return;
    }
    if (event.repeat || event.metaKey || event.ctrlKey || event.shiftKey) return;
    beginHold();
}

export function onVoiceKeyUp(event: KeyboardEvent): void {
    if (event.code === HOLD_KEY) endHold();
}

/** Hold right Option to dictate into the focused agent or terminal. */
export function useVoiceDictation(): void {
    const enabled = useStore((s) => s.voiceDictation);

    useEffect(() => {
        if (!enabled) return;
        const controller = new AbortController();
        inOrder(async () => {
            await voiceApi.subscribe(handleVoiceEvent, controller.signal);
            if (!controller.signal.aborted) await prepare();
        });
        window.addEventListener("keydown", onVoiceKeyDown, { capture: true, signal: controller.signal });
        window.addEventListener("keyup", onVoiceKeyUp, { capture: true, signal: controller.signal });
        window.addEventListener("blur", abandonHold, { signal: controller.signal });
        return () => {
            controller.abort();
            inOrder(shutdown);
        };
    }, [enabled]);
}
