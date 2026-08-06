import { useEffect } from "react";
import { isPermissionGranted, onAction, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getState, useStore } from "../state/store";
import { notify } from "../state/toast";
import { shouldNotifyAgent } from "../notifications/policy";
import * as cmd from "../state/commands";

const timers = new Map<string, number>();

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

function playSignal(state: "blocked" | "done"): void {
    try {
        const audio = new AudioContext();
        const oscillator = audio.createOscillator();
        const gain = audio.createGain();
        oscillator.type = state === "blocked" ? "square" : "sine";
        oscillator.frequency.value = state === "blocked" ? 330 : 660;
        gain.gain.setValueAtTime(0.045, audio.currentTime);
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
                if (current.state !== "blocked" && current.state !== "done") continue;
                const agent = state.agents[agentId];
                const sessionId = state.sessionOrder.find((id) => state.agentsBySession[id]?.includes(agentId));
                if (!agent || !sessionId) continue;
                const timer = window.setTimeout(async () => {
                    timers.delete(agentId);
                    const latest = getState();
                    const runtime = latest.agentActivity[agentId];
                    if (!runtime || runtime.state !== current.state) return;
                    const focused = await getCurrentWindow()
                        .isFocused()
                        .catch(() => document.hasFocus());
                    if (!shouldNotifyAgent(runtime.state, latest.notificationPreferences, agent.type, focused)) return;
                    const stateLabel = runtime.state === "blocked" ? "needs your input" : "is done";
                    if (latest.notificationPreferences.sounds) playSignal(runtime.state === "blocked" ? "blocked" : "done");
                    notify(runtime.state === "blocked" ? "error" : "success", `${agent.title} ${stateLabel}`, {
                        action: { label: "Focus", run: () => focusAgent(sessionId, agentId) },
                    });
                    if (await isPermissionGranted().catch(() => false)) {
                        sendNotification({
                            title: runtime.state === "blocked" ? "Agent needs input" : "Agent finished",
                            body: `${agent.title} · ${latest.sessions[sessionId]?.name ?? "project"}`,
                            group: "sikemux-agents",
                            autoCancel: true,
                            extra: { sessionId, agentId },
                        });
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
