import { StreamLanguage, type Extension, type StreamParser } from "../../plugin-api/editor";
import type { Engine } from "./api";

const KEYWORDS = new Set(
    (
        "add all alter analyze and any as asc begin between by case cast check column commit constraint create cross " +
        "database default delete desc distinct drop else end except exists explain false fetch for foreign from full " +
        "grant group having if ilike in index inner insert intersect into is join key left like limit natural not null " +
        "offset on or order outer over partition primary references returning revoke right rollback row rows schema " +
        "select set table then to transaction true truncate union unique update using values view when where window with"
    ).split(" "),
);

const TYPES = new Set(
    (
        "bigint bigserial binary blob bool boolean bytea char character date datetime decimal double float int integer " +
        "interval json jsonb money numeric real serial smallint text time timestamp timestamptz tinyint uuid varchar"
    ).split(" "),
);

export type SqlStyle = "keyword" | "typeName" | "string" | "comment" | "number" | "operator" | "variableName" | "punctuation" | null;

/** What carries over from one line to the next: an open block comment, or a PostgreSQL `$tag$` block. */
export interface SqlState {
    inComment: boolean;
    dollarTag: string | null;
}

export const startState = (): SqlState => ({ inComment: false, dollarTag: null });

/** The style of the token that starts at `at`, and where it ends. */
export function nextToken(line: string, at: number, state: SqlState, engine: Engine): { end: number; style: SqlStyle } {
    if (state.inComment) {
        const close = line.indexOf("*/", at);
        if (close === -1) return { end: line.length, style: "comment" };
        state.inComment = false;
        return { end: close + 2, style: "comment" };
    }
    if (state.dollarTag) {
        const close = line.indexOf(state.dollarTag, at);
        if (close === -1) return { end: line.length, style: "string" };
        const end = close + state.dollarTag.length;
        state.dollarTag = null;
        return { end, style: "string" };
    }
    const rest = line.slice(at);
    const char = line[at];
    if (rest.startsWith("--") || (engine === "mysql" && char === "#")) return { end: line.length, style: "comment" };
    if (rest.startsWith("/*")) {
        state.inComment = true;
        return nextToken(line, at + 2, state, engine);
    }
    if (engine === "postgres" && char === "$") {
        const tag = /^\$[A-Za-z_]*\$/.exec(rest)?.[0];
        if (tag) {
            state.dollarTag = tag;
            return nextToken(line, at + tag.length, state, engine);
        }
    }
    if (char === "'" || char === '"' || char === "`") return { end: quoted(line, at, char), style: char === "'" ? "string" : "variableName" };
    const number = /^(?:\d+\.?\d*(?:e[+-]?\d+)?|\.\d+)/i.exec(rest);
    if (number) return { end: at + number[0].length, style: "number" };
    const word = /^[A-Za-z_][A-Za-z0-9_$]*/.exec(rest);
    if (word) {
        const lower = word[0].toLowerCase();
        const style = KEYWORDS.has(lower) ? "keyword" : TYPES.has(lower) ? "typeName" : null;
        return { end: at + word[0].length, style };
    }
    if (/^[-+*/%=<>!|&^~:]/.test(rest)) return { end: at + (/^[-+*/%=<>!|&^~:]+/.exec(rest)?.[0].length ?? 1), style: "operator" };
    if (/^[(),;.[\]]/.test(rest)) return { end: at + 1, style: "punctuation" };
    return { end: at + 1, style: null };
}

/** Past the quote that closes the one at `open`; a doubled quote or a backslash escapes it, and a string may run off the line. */
function quoted(line: string, open: number, quote: string): number {
    let at = open + 1;
    while (at < line.length) {
        if (line[at] === "\\") at += 2;
        else if (line[at] === quote && line[at + 1] === quote) at += 2;
        else if (line[at] === quote) return at + 1;
        else at++;
    }
    return line.length;
}

function parser(engine: Engine): StreamParser<SqlState> {
    return {
        name: "sql",
        startState,
        copyState: (state) => ({ ...state }),
        token(stream, state) {
            if (stream.eatSpace()) return null;
            const { end, style } = nextToken(stream.string, stream.pos, state, engine);
            stream.pos = Math.max(end, stream.pos + 1);
            return style;
        },
        languageData: { commentTokens: { line: "--", block: { open: "/*", close: "*/" } } },
    };
}

/** SQL highlighting for one engine: the common keywords and types, and how that engine quotes and comments. */
export function sqlHighlighting(engine: Engine): Extension {
    return StreamLanguage.define(parser(engine));
}
