import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import { agentIdsOf } from "./state/selectors";
import { swallow } from "./state/toast";
import { getState, setState, useStore, type StoreState } from "./state/store";

type AgentView = Pick<StoreState, "sessionOrder" | "sessions" | "windows" | "windowsBySession" | "agents" | "agentActivity">;

export interface AgentAlert {
    agentId: string;
    title: string;
    body: string;
}

function* openAgents(state: AgentView) {
    for (const sessionId of state.sessionOrder) {
        const session = state.sessions[sessionId];
        if (!session) continue;
        for (const agentId of agentIdsOf(state, sessionId)) {
            const agent = state.agents[agentId];
            if (agent) yield { agentId, agent, sessionName: session.name };
        }
    }
}

/** Agents that are waiting on the person: asking for input, or finished while unseen. */
export function agentsNeedingYou(state: AgentView): number {
    let count = 0;
    for (const { agentId } of openAgents(state)) {
        const status = state.agentActivity[agentId]?.state;
        if (status === "blocked" || status === "done") count += 1;
    }
    return count;
}

/**
 * One alert per agent that has just asked for input or just finished working.
 * This reads what the agent itself reported, not what the rail shows, because
 * the rail never marks the agent on screen as finished, and that is often the
 * one someone left running while they switched to another app.
 */
export function newAgentAlerts(previous: AgentView, next: AgentView): AgentAlert[] {
    const alerts: AgentAlert[] = [];
    for (const { agentId, agent, sessionName } of openAgents(next)) {
        const now = next.agentActivity[agentId]?.backendState;
        const before = previous.agentActivity[agentId]?.backendState;
        if (now === before) continue;
        if (now === "blocked") alerts.push({ agentId, title: `${agent.title} needs your input`, body: sessionName });
        else if (now === "idle" && (before === "working" || before === "blocked"))
            alerts.push({ agentId, title: `${agent.title} finished`, body: sessionName });
    }
    return alerts;
}

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

async function post(title: string, body: string): Promise<void> {
    if ((await invoke<string>("plugin:notification|request_permission")) !== "granted") return;
    await invoke("plugin:notification|notify", { options: { title, body } });
    playBip();
}

export function sendTestNotification(): void {
    post("Sikemux notifications are on", "This is how an agent will tell you it needs you or has finished.").catch(swallow("test notification"));
}

/**
 * macOS asks whether an app may notify only the first time it posts one, so a
 * first launch posts a greeting to put that question up front instead of in
 * the middle of an agent's work.
 */
export function introduceNotifications(): void {
    const state = getState();
    if (!state.agentNotifications || state.notificationsIntroduced) return;
    setState({ notificationsIntroduced: true });
    sendTestNotification();
}

/**
 * Keeps the Dock badge at the number of agents waiting on the person and, while
 * Sikemux is in the background, posts a notification and bounces the Dock icon
 * as each one starts waiting.
 */
export function installAgentNotifications(hasFocus: () => boolean = () => document.hasFocus()): () => void {
    let badge = 0;
    // The webview keeps audio silent until the page is clicked, so the first click readies it for later bips.
    const wakeAudio = () => void audioContext().resume();
    window.addEventListener("pointerdown", wakeAudio, { capture: true, once: true });
    const unsubscribe = useStore.subscribe((next, previous) => {
        if (next.agentActivity === previous.agentActivity && next.agents === previous.agents) return;
        const count = agentsNeedingYou(next);
        if (count !== badge) {
            badge = count;
            getCurrentWindow()
                .setBadgeCount(count > 0 ? count : undefined)
                .catch(swallow("set dock badge"));
        }
        if (!next.agentNotifications || hasFocus()) return;
        const alerts = newAgentAlerts(previous, next);
        if (alerts.length === 0) return;
        getCurrentWindow().requestUserAttention(UserAttentionType.Informational).catch(swallow("bounce dock icon"));
        for (const alert of alerts) post(alert.title, alert.body).catch(swallow("agent notification"));
    });
    return () => {
        window.removeEventListener("pointerdown", wakeAudio, { capture: true });
        unsubscribe();
    };
}

export function useAgentNotifications(): void {
    useEffect(() => installAgentNotifications(), []);
}
