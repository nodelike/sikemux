import type { NotificationPreferences } from "../state/types";

function minutes(value: string): number {
    const [hour, minute] = value.split(":").map(Number);
    return hour * 60 + minute;
}

export function inQuietHours(preferences: NotificationPreferences, date = new Date()): boolean {
    if (!preferences.quietHoursEnabled) return false;
    const now = date.getHours() * 60 + date.getMinutes();
    const start = minutes(preferences.quietHoursStart);
    const end = minutes(preferences.quietHoursEnd);
    if (start === end) return true;
    return start < end ? now >= start && now < end : now >= start || now < end;
}

export function shouldNotifyAgent(state: string, preferences: NotificationPreferences, agentType: string, date = new Date()): boolean {
    return (
        preferences.enabled &&
        (state === "blocked" || state === "done") &&
        !preferences.mutedAgentTypes.includes(agentType as never) &&
        !inQuietHours(preferences, date)
    );
}

export function shouldSendNativeAgentNotification(
    state: string,
    preferences: NotificationPreferences,
    agentType: string,
    appFocused: boolean,
    date = new Date(),
): boolean {
    return shouldNotifyAgent(state, preferences, agentType, date) && !(preferences.onlyWhenUnfocused && appFocused);
}
