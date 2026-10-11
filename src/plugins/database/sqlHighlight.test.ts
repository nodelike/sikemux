import { describe, expect, it } from "vitest";
import type { Engine } from "./api";
import { nextToken, startState, type SqlState, type SqlStyle } from "./sqlHighlight";

/** Every token on each line with its style, spaces left out, carrying state across lines as the editor does. */
function tokens(text: string, engine: Engine = "postgres", state: SqlState = startState()): [string, SqlStyle][] {
    const found: [string, SqlStyle][] = [];
    for (const line of text.split("\n")) {
        let at = 0;
        while (at < line.length) {
            if (line[at] === " " && !state.inComment && !state.dollarTag) {
                at++;
                continue;
            }
            const { end, style } = nextToken(line, at, state, engine);
            found.push([line.slice(at, end), style]);
            at = Math.max(end, at + 1);
        }
    }
    return found;
}

describe("sqlHighlight", () => {
    it("tells keywords, types, names, numbers and operators apart in any case", () => {
        expect(tokens("SELECT id, total::numeric FROM orders WHERE total >= 10.5")).toEqual([
            ["SELECT", "keyword"],
            ["id", null],
            [",", "punctuation"],
            ["total", null],
            ["::", "operator"],
            ["numeric", "typeName"],
            ["FROM", "keyword"],
            ["orders", null],
            ["WHERE", "keyword"],
            ["total", null],
            [">=", "operator"],
            ["10.5", "number"],
        ]);
    });

    it("reads strings with doubled quotes and quoted names", () => {
        expect(tokens(`select 'it''s', "Odd Name"`)).toEqual([
            ["select", "keyword"],
            ["'it''s'", "string"],
            [",", "punctuation"],
            ['"Odd Name"', "variableName"],
        ]);
        expect(tokens("select `odd name` from t", "mysql")[1]).toEqual(["`odd name`", "variableName"]);
    });

    it("carries a block comment across lines", () => {
        expect(tokens("select /* one\ntwo */ 1")).toEqual([
            ["select", "keyword"],
            ["/* one", "comment"],
            ["two */", "comment"],
            ["1", "number"],
        ]);
    });

    it("knows each engine's own comments and blocks", () => {
        expect(tokens("select 1 -- note")[2]).toEqual(["-- note", "comment"]);
        expect(tokens("select 1 # note", "mysql")[2]).toEqual(["# note", "comment"]);
        expect(tokens("select 1 # note", "postgres")[2]).toEqual(["#", null]);
        expect(tokens("as $body$ begin;\nend; $body$ language")).toEqual([
            ["as", "keyword"],
            ["$body$ begin;", "string"],
            ["end; $body$", "string"],
            ["language", null],
        ]);
    });
});
