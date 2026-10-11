import { describe, expect, it } from "vitest";
import { previewSql, quoteIdentifier, splitStatements, statementAt } from "./sql";

const texts = (script: string) => splitStatements(script).map((statement) => statement.text);

describe("sql", () => {
    it("splits a script on semicolons and drops empty statements", () => {
        expect(texts("select 1; select 2;\n\n;  ")).toEqual(["select 1", "select 2"]);
        expect(texts("  select 1  ")).toEqual(["select 1"]);
        expect(texts("")).toEqual([]);
    });

    it("keeps semicolons inside quotes, identifiers and comments", () => {
        expect(texts("select 'a;b', \"c;d\", `e;f`; select 2")).toEqual(["select 'a;b', \"c;d\", `e;f`", "select 2"]);
        expect(texts("select 'it''s; fine'; select 2")).toEqual(["select 'it''s; fine'", "select 2"]);
        expect(texts("select 1 -- not; here\n; select 2")).toEqual(["select 1 -- not; here", "select 2"]);
        expect(texts("select /* a; b */ 1; select 2")).toEqual(["select /* a; b */ 1", "select 2"]);
    });

    it("keeps a PostgreSQL function body whole", () => {
        const script = "create function f() returns int as $body$ begin return 1; end; $body$ language plpgsql; select f()";
        expect(texts(script)).toEqual(["create function f() returns int as $body$ begin return 1; end; $body$ language plpgsql", "select f()"]);
        expect(texts("select $$a;b$$; select 2")).toEqual(["select $$a;b$$", "select 2"]);
        expect(texts("select $1 from t; select 2")).toEqual(["select $1 from t", "select 2"]);
    });

    it("records where each statement sits", () => {
        const [first, second] = splitStatements("  select 1;\n select 22;");
        expect(first).toEqual({ from: 2, to: 10, text: "select 1" });
        expect(second).toEqual({ from: 13, to: 22, text: "select 22" });
    });

    it("finds the statement under the cursor, or the one just before", () => {
        const script = "select 1;\nselect 2;\n\n";
        expect(statementAt(script, 3)?.text).toBe("select 1");
        expect(statementAt(script, 9)?.text).toBe("select 1");
        expect(statementAt(script, 12)?.text).toBe("select 2");
        expect(statementAt(script, script.length)?.text).toBe("select 2");
        expect(statementAt("", 0)).toBeNull();
    });

    it("quotes identifiers the way each engine reads them", () => {
        expect(quoteIdentifier("postgres", 'odd "name"')).toBe('"odd ""name"""');
        expect(quoteIdentifier("mysql", "odd `name`")).toBe("`odd ``name```");
        expect(previewSql("postgres", "public", "orders")).toBe('select * from "public"."orders" limit 100;');
        expect(previewSql("mysql", "shop", "orders", 10)).toBe("select * from `shop`.`orders` limit 10;");
        expect(previewSql("sqlite", "", "orders")).toBe('select * from "orders" limit 100;');
    });
});
