import { describe, expect, it } from "vitest";
import { calendarColumns, weekday } from "./activityCalendar";

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
});
