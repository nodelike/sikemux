import { describe, expect, it } from "vitest";
import { closeOtherTabs, closeProfileTabs, closeTab, consoleFor, consoleTab, historyTab, newConsole, openTab, tableTab, type TabStrip } from "./tabs";

const empty: TabStrip = { tabs: [], active: null };
const ids = (strip: TabStrip) => strip.tabs.map((tab) => tab.id);

describe("database tabs", () => {
    it("opens a new tab after the one in front, and only brings an open one forward", () => {
        let strip = openTab(empty, consoleTab("p1", 1));
        strip = openTab(strip, historyTab("p1"));
        strip = { ...strip, active: "console:p1:1" };
        strip = openTab(strip, tableTab("p2", "public", "orders"));
        expect(ids(strip)).toEqual(["console:p1:1", "table:p2:public.orders", "history:p1"]);
        expect(strip.active).toBe("table:p2:public.orders");

        const again = openTab(strip, historyTab("p1"));
        expect(ids(again)).toEqual(ids(strip));
        expect(again.active).toBe("history:p1");
    });

    it("hands the front to the right-hand neighbour when the front tab closes, else the left", () => {
        const strip: TabStrip = { tabs: [consoleTab("p1", 1), historyTab("p1"), consoleTab("p2", 1)], active: "history:p1" };
        expect(closeTab(strip, "history:p1").active).toBe("console:p2:1");
        expect(closeTab({ ...strip, active: "console:p2:1" }, "console:p2:1").active).toBe("history:p1");
        expect(closeTab(strip, "console:p1:1").active).toBe("history:p1");
        expect(closeTab({ tabs: [historyTab("p1")], active: "history:p1" }, "history:p1")).toEqual(empty);
    });

    it("closes the others, or every tab of a removed connection", () => {
        const strip: TabStrip = { tabs: [consoleTab("p1", 1), historyTab("p2"), tableTab("p1", "main", "t")], active: "console:p1:1" };
        expect(closeOtherTabs(strip, "history:p2")).toEqual({ tabs: [historyTab("p2")], active: "history:p2" });
        expect(closeProfileTabs(strip, "p1")).toEqual({ tabs: [historyTab("p2")], active: "history:p2" });
    });

    it("reuses a connection's console, and numbers new ones after the highest", () => {
        const strip: TabStrip = { tabs: [consoleTab("p1", 1), consoleTab("p1", 3), consoleTab("p2", 1)], active: "console:p1:3" };
        expect(consoleFor(strip, "p1").id).toBe("console:p1:3");
        expect(consoleFor({ ...strip, active: null }, "p1").id).toBe("console:p1:1");
        expect(consoleFor(strip, "p9").id).toBe("console:p9:1");
        expect(newConsole(strip, "p1").id).toBe("console:p1:4");
        expect(newConsole(strip, "p9").id).toBe("console:p9:1");
    });
});
