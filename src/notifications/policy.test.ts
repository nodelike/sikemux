import { describe, expect, it } from "vitest";
import { inQuietHours, shouldNotifyAgent } from "./policy";
import type { NotificationPreferences } from "../state/types";

const prefs: NotificationPreferences = {
    enabled: true,
    onlyWhenUnfocused: true,
    sounds: true,
    soundStyle: "soft",
    delayMs: 650,
    quietHoursEnabled: true,
    quietHoursStart: "22:00",
    quietHoursEnd: "08:00",
    mutedAgentTypes: [],
};

describe("agent notification policy", () => {
    it("handles quiet hours across midnight", () => {
        expect(inQuietHours(prefs, new Date(2025, 0, 1, 23, 0))).toBe(true);
        expect(inQuietHours(prefs, new Date(2025, 0, 1, 7, 59))).toBe(true);
        expect(inQuietHours(prefs, new Date(2025, 0, 1, 12, 0))).toBe(false);
    });
    it("only alerts for attention states while unfocused", () => {
        const midday = new Date(2025, 0, 1, 12, 0);
        expect(shouldNotifyAgent("blocked", prefs, "claude", false, midday)).toBe(true);
        expect(shouldNotifyAgent("done", prefs, "claude", true, midday)).toBe(false);
        expect(shouldNotifyAgent("working", prefs, "claude", false, midday)).toBe(false);
    });
});
