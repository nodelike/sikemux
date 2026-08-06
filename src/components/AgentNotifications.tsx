import { useEffect } from "react";
import { isPermissionGranted, onAction, requestPermission, type Options } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getState, useStore } from "../state/store";
import { notify } from "../state/toast";
import { shouldNotifyAgent, shouldSendNativeAgentNotification } from "../notifications/policy";
import * as cmd from "../state/commands";
import type { AgentRuntimeState } from "../state/types";

const timers = new Map<string, number>();

export type AgentNotificationState = "blocked" | "done";

export function notificationStateForTransition(current: AgentRuntimeState, previous: AgentRuntimeState | undefined): AgentNotificationState | null {
    if (current.backendState === "blocked" && previous?.backendState !== "blocked") return "blocked";
    if (current.backendState === "idle" && (previous?.backendState === "working" || previous?.backendState === "blocked")) return "done";
    return null;
}

export async function requestAgentNotificationPermission(): Promise<boolean> {
    if (await isPermissionGranted()) return true;
    return (await requestPermission()) === "granted";
}

async function focusAgent(sessionId: string, agentId: string): Promise<void> {
    cmd.selectSession(sessionId);
    cmd.selectAgent(agentId);
    const window = getCurrentWindow();
    await window.show();
    await window.unminimize();
    await window.setFocus();
}

export function sendAgentSystemNotification(options: Options, sessionId: string, agentId: string): void {
    // The plugin's desktop `sendNotification` is a thin wrapper around the Web
    // Notification constructor. Construct it here so its click callback can
    // return to the exact agent. Keep `extra` on the options for the plugin's
    // mobile action listener below.
    const notification = new window.Notification(options.title, options as NotificationOptions);
    notification.onclick = () => {
        notification.close();
        void focusAgent(sessionId, agentId);
    };
}

function playSignal(state: "blocked" | "done", style: "soft" | "bright"): void {
    try {
        const audio = new AudioContext();
        const oscillator = audio.createOscillator();
        const gain = audio.createGain();
        oscillator.type = style === "bright" ? "triangle" : "sine";
        oscillator.frequency.value = (state === "blocked" ? 330 : 660) * (style === "bright" ? 1.25 : 1);
        gain.gain.setValueAtTime(style === "bright" ? 0.065 : 0.035, audio.currentTime);
        gain.gain.exponentialRampToValueAtTime(0.001, audio.currentTime + 0.16);
        oscillator.connect(gain).connect(audio.destination);
        oscillator.start();
        oscillator.stop(audio.currentTime + 0.17);
        oscillator.addEventListener("ended", () => void audio.close());
    } catch {
        // WebAudio may be unavailable before the first user gesture.
    }
}

export function AgentNotifications() {
    const enabled = useStore((s) => s.notificationPreferences.enabled);
    useEffect(() => {
        if (!enabled) for (const timer of timers.values()) window.clearTimeout(timer);
    }, [enabled]);

    useEffect(() => {
        const unsubscribe = useStore.subscribe((state, previous) => {
            for (const [agentId, current] of Object.entries(state.agentActivity)) {
                if (current === previous.agentActivity[agentId]) continue;
                const oldTimer = timers.get(agentId);
                if (oldTimer != null) {
                    window.clearTimeout(oldTimer);
                    timers.delete(agentId);
                }
                const notificationState = notificationStateForTransition(current, previous.agentActivity[agentId]);
                if (!notificationState) continue;
                const agent = state.agents[agentId];
                const sessionId = state.sessionOrder.find((id) => state.agentsBySession[id]?.includes(agentId));
                if (!agent || !sessionId) continue;
                const timer = window.setTimeout(async () => {
                    timers.delete(agentId);
                    const latest = getState();
                    const runtime = latest.agentActivity[agentId];
                    if (!runtime || runtime.sequence !== current.sequence) return;
                    if (!shouldNotifyAgent(notificationState, latest.notificationPreferences, agent.type)) return;
                    const stateLabel = notificationState === "blocked" ? "needs your input" : "is done";
                    if (latest.notificationPreferences.sounds) playSignal(notificationState, latest.notificationPreferences.soundStyle);
                    notify(notificationState === "blocked" ? "error" : "success", `${agent.title} ${stateLabel}`, {
                        action: { label: "Focus", run: () => focusAgent(sessionId, agentId) },
                    });
                    const focused = await getCurrentWindow()
                        .isFocused()
                        .catch(() => document.hasFocus());
                    if (
                        shouldSendNativeAgentNotification(notificationState, latest.notificationPreferences, agent.type, focused) &&
                        (await isPermissionGranted().catch(() => false))
                    ) {
                        sendAgentSystemNotification(
                            {
                                title: notificationState === "blocked" ? "Agent needs input" : "Agent finished",
                                body: `${agent.title} · ${latest.sessions[sessionId]?.name ?? "project"}`,
                                group: "sikemux-agents",
                                autoCancel: true,
                                extra: { sessionId, agentId },
                            },
                            sessionId,
                            agentId,
                        );
                    }
                }, state.notificationPreferences.delayMs);
                timers.set(agentId, timer);
            }
        });
        const action = onAction((notification) => {
            const sessionId = notification.extra?.sessionId;
            const agentId = notification.extra?.agentId;
            if (typeof sessionId === "string" && typeof agentId === "string") void focusAgent(sessionId, agentId);
        });
        return () => {
            unsubscribe();
            void action.then((listener) => listener.unregister());
            for (const timer of timers.values()) window.clearTimeout(timer);
            timers.clear();
        };
    }, []);
    return null;
}
