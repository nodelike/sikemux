import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import { getIpcTransport } from "../api/transport";
import { swallow } from "../state/toast";
import { getState, useStore, type StoreState } from "../state/store";
import { setPreference } from "../state/preferences";

let audio: AudioContext | null = null;
const audioContext = (): AudioContext => (audio ??= new AudioContext());

/** A short, soft tone, so a notification is heard whatever macOS's sound settings say. */
export function playBip(): void {
    const context = audioContext();
    void context.resume();
    const start = context.currentTime;
    const tone = context.createOscillator();
    const volume = context.createGain();
    tone.frequency.value = 880;
    volume.gain.setValueAtTime(0.0001, start);
    volume.gain.exponentialRampToValueAtTime(0.2, start + 0.01);
    volume.gain.exponentialRampToValueAtTime(0.0001, start + 0.15);
    tone.connect(volume).connect(context.destination);
    tone.start(start);
    tone.stop(start + 0.16);
}

export async function postNotification(title: string, body: string, agentId?: string): Promise<void> {
    await invoke("post_notification", { title, body, agentId });
    playBip();
}

export function sendTestNotification(): void {
    postNotification("Sikemux notifications are on", "This is how an agent will tell you it needs you or has finished.").catch(
        swallow("test notification"),
    );
}

/**
 * macOS asks whether an app may notify only the first time it posts one, so a
 * first launch posts a greeting to put that question up front instead of in
 * the middle of an agent's work.
 */
export function introduceNotifications(): void {
    const state = getState();
    if (!state.preferences?.agentNotifications || state.preferences.notificationsIntroduced) return;
    setPreference("notificationsIntroduced", true);
    sendTestNotification();
}

/** What the core tells the window to say, about the agent it names. */
export interface AgentNotice {
    title: string;
    body: string;
    agentId: string;
}

/** Agents asking for input plus agents that finished while unseen, as the core groups them. */
export function agentsNeedingYou(agentStatus: StoreState["agentStatus"]): number {
    return Object.values(agentStatus).filter((shown) => shown.group === "blocked" || shown.group === "done").length;
}

/**
 * Keeps the Dock badge at the number of agents waiting on the person and, while
 * Sikemux is in the background, posts each notice the core sends and bounces the Dock icon.
 */
export function installAgentNotifications(hasFocus: () => boolean = () => document.hasFocus()): () => void {
    let badge = 0;
    // The webview keeps audio silent until the page is clicked, so the first click readies it for later bips.
    const wakeAudio = () => void audioContext().resume();
    window.addEventListener("pointerdown", wakeAudio, { capture: true, once: true });
    const unsubscribe = useStore.subscribe((next, previous) => {
        if (next.agentStatus === previous.agentStatus) return;
        const count = agentsNeedingYou(next.agentStatus);
        if (count === badge) return;
        badge = count;
        getCurrentWindow()
            .setBadgeCount(count > 0 ? count : undefined)
            .catch(swallow("set dock badge"));
    });
    const controller = new AbortController();
    getIpcTransport()
        .subscribe<AgentNotice>(
            "agent_notice",
            ({ payload }) => {
                if (!getState().preferences?.agentNotifications || hasFocus()) return;
                getCurrentWindow().requestUserAttention(UserAttentionType.Informational).catch(swallow("bounce dock icon"));
                postNotification(payload.title, payload.body, payload.agentId).catch(swallow("agent notification"));
            },
            { signal: controller.signal },
        )
        .catch(swallow("agent notices"));
    return () => {
        window.removeEventListener("pointerdown", wakeAudio, { capture: true });
        controller.abort();
        unsubscribe();
    };
}

export function useAgentNotifications(): void {
    useEffect(() => installAgentNotifications(), []);
}
