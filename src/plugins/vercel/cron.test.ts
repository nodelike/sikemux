import { describe, expect, it } from "vitest";
import { describeCron } from "./cron";

describe("a cron schedule in words", () => {
    it("reads the shapes people write most", () => {
        expect(describeCron("* * * * *")).toBe("Every minute");
        expect(describeCron("*/15 * * * *")).toBe("Every 15 minutes");
        expect(describeCron("5 * * * *")).toBe("Every hour at :05");
        expect(describeCron("0 */6 * * *")).toBe("Every 6 hours at :00");
        expect(describeCron("0 9 * * *")).toBe("Every day at 09:00 UTC");
        expect(describeCron("30 7 * * 1-5")).toBe("Weekdays at 07:30 UTC");
        expect(describeCron("0 0 * * 1")).toBe("Every Monday at 00:00 UTC");
        expect(describeCron("0 0 * * 7")).toBe("Every Sunday at 00:00 UTC");
        expect(describeCron("0 3 1 * *")).toBe("Monthly on day 1 at 03:00 UTC");
    });

    it("shows anything else as written", () => {
        expect(describeCron("0 9 * 1,7 *")).toBe("0 9 * 1,7 *");
        expect(describeCron("@daily")).toBe("@daily");
    });
});
