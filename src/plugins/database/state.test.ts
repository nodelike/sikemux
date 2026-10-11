import { describe, expect, it } from "vitest";
import { readQuery, updateQuery } from "./queryState";
import { closeConnectionTabs, closeDatabaseTab, readDatabaseView as readView, showTab, updateDatabaseView } from "./state";
import { consoleTab, historyTab, tableTab } from "./tabs";

let pane = 0;
const nextPane = () => `state-pane-${++pane}`;

describe("database pane state", () => {
    it("opens tabs, closes the form when one is shown, and forgets a closed console's SQL", () => {
        const paneId = nextPane();
        updateDatabaseView(paneId, { editing: "new" });
        showTab(paneId, consoleTab("p1", 1));
        showTab(paneId, tableTab("p1", "public", "orders"));
        updateQuery("console:p1:1", { sql: "select 1" });
        expect(readView(paneId)).toMatchObject({ active: "table:p1:public.orders", editing: null });

        closeDatabaseTab(paneId, "console:p1:1");
        expect(readQuery("console:p1:1").sql).toBe("");
        expect(readView(paneId).tabs.map((tab) => tab.id)).toEqual(["table:p1:public.orders"]);
    });

    it("closes every tab of a connection", () => {
        const paneId = nextPane();
        showTab(paneId, consoleTab("p1", 1));
        showTab(paneId, historyTab("p2"));
        showTab(paneId, tableTab("p1", "main", "t"));
        closeConnectionTabs(paneId, "p1");
        expect(readView(paneId)).toMatchObject({ tabs: [historyTab("p2")], active: "history:p2" });
    });
});
