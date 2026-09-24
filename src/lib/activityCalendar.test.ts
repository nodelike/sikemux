import { describe, expect, it } from "vitest";
import { calendarColumns, levelOf, levelThresholds, streaks, weekday } from "./activityCalendar";

const day = (iso: string) => Date.parse(`${iso}T00:00:00Z`) / 86_400_000;

describe("activity calendar", () => {
    it("starts weeks on Monday", () => {
        expect(weekday(day("2026-09-21"))).toBe(0);
        expect(weekday(day("2026-09-27"))).toBe(6);
    });

    it("ends on today's column and leaves the rest of the week empty", () => {
        const today = day("2026-09-25");
        const columns = calendarColumns(today);
        expect(columns).toHaveLength(53);
        const last = columns.at(-1)!.days;
        expect(last[weekday(today)]).toBe(today);
        expect(last.slice(weekday(today) + 1).every((cell) => cell === null)).toBe(true);
        expect(columns[0].days[0]).toBe(today - weekday(today) - 52 * 7);
    });

    it("labels each month once", () => {
        const months = calendarColumns(day("2026-09-25"))
            .map((column) => column.month)
            .filter(Boolean);
        expect(new Set(months).size).toBe(months.length);
    });

    it("grades days by the quartiles of active days", () => {
        const thresholds = levelThresholds([0, 1, 2, 3, 4, 100]);
        expect(levelOf(0, thresholds)).toBe(0);
        expect(levelOf(1, thresholds)).toBe(1);
        expect(levelOf(100, thresholds)).toBe(4);
    });

    it("keeps a streak alive until today is over", () => {
        const today = 100;
        expect(streaks(new Set([90, 91, 92, 98, 99]), today)).toEqual({ activeDays: 5, current: 2, longest: 3 });
        expect(streaks(new Set([97, 98]), today).current).toBe(0);
    });
});
