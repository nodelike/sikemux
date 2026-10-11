import { describe, expect, it } from "vitest";
import type { ResultSet } from "./api";
import { ago, cellText, resultMessage, toCsv, duration, mainResult, summary, toMarkdown, toTsv } from "./results";

const customers: ResultSet = {
    columns: [
        { name: "id", type: "int4", numeric: true },
        { name: "name", type: "text", numeric: false },
        { name: "note", type: "text", numeric: false },
    ],
    rows: [
        [1, "Ada", "likes | pipes"],
        [2, "Linus", null],
    ],
    truncated: false,
    affected: null,
};

const changed: ResultSet = { columns: [], rows: [], truncated: false, affected: 3 };

describe("results", () => {
    it("shows each kind of cell", () => {
        expect(cellText(null)).toBe("NULL");
        expect(cellText(true)).toBe("true");
        expect(cellText(42.5)).toBe("42.5");
        expect(cellText("9223372036854775807")).toBe("9223372036854775807");
    });

    it("sums up rows returned, rows changed and rows left out", () => {
        expect(summary(customers)).toBe("2 rows");
        expect(summary(changed)).toBe("3 rows changed");
        expect(summary({ ...changed, affected: 1 })).toBe("1 row changed");
        expect(summary({ ...customers, truncated: true })).toBe("First 2 rows; more were left out");
    });

    it("writes durations the way people read them", () => {
        expect(duration(12)).toBe("12 ms");
        expect(duration(1530)).toBe("1.53 s");
        expect(duration(42_000)).toBe("42.0 s");
        expect(duration(125_000)).toBe("2 min 5 s");
    });

    it("shows the last result that returned rows", () => {
        expect(mainResult({ results: [customers, changed], millis: 1 })).toBe(0);
        expect(mainResult({ results: [changed, customers], millis: 1 })).toBe(1);
        expect(mainResult({ results: [changed, changed], millis: 1 })).toBe(1);
        expect(mainResult({ results: [], millis: 1 })).toBe(0);
    });

    it("says how long ago a run was", () => {
        const now = Date.UTC(2026, 9, 4, 12, 0, 0);
        expect(ago(now - 20_000, now)).toBe("just now");
        expect(ago(now - 5 * 60_000, now)).toBe("5 min ago");
        expect(ago(now - 3 * 3_600_000, now)).toBe("3 h ago");
        expect(ago(now - 26 * 3_600_000, now)).toBe("yesterday");
        expect(ago(now - 3 * 86_400_000, now)).toBe("3 days ago");
        expect(ago(now - 30 * 86_400_000, now)).toBe("Sep 4");
    });

    it("copies as tab-separated text with empty cells for null", () => {
        expect(toTsv(customers)).toBe("id\tname\tnote\n1\tAda\tlikes | pipes\n2\tLinus\t");
    });

    it("hands an agent the database, the SQL and the table", () => {
        const message = resultMessage("Shop", "PostgreSQL", "select id, name, note from customers", customers);
        expect(message.startsWith('Results of a query on the PostgreSQL database "Shop":')).toBe(true);
        expect(message).toContain("```sql\nselect id, name, note from customers\n```");
        expect(message).toContain("| 2 | Linus | NULL |");
    });

    it("copies as CSV, quoting what needs it and leaving NULL empty", () => {
        const tricky: ResultSet = {
            ...customers,
            rows: [
                [1, 'say "hi", ok', "two\nlines"],
                [2, "plain", null],
            ],
        };
        expect(toCsv(tricky)).toBe('id,name,note\r\n1,"say ""hi"", ok","two\nlines"\r\n2,plain,');
    });

    it("writes a markdown table for an agent, numbers to the right and pipes escaped", () => {
        expect(toMarkdown(customers)).toBe(
            ["| id | name | note |", "| ---: | --- | --- |", "| 1 | Ada | likes \\| pipes |", "| 2 | Linus | NULL |"].join("\n"),
        );
        expect(toMarkdown(customers, 1)).toContain("2 rows; 1 more row not shown");
        expect(toMarkdown(changed)).toBe("3 rows changed");
    });
});
