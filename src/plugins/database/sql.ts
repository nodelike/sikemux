import type { Engine } from "./api";

export interface Statement {
    from: number;
    to: number;
    text: string;
}

/**
 * The statements in a script, split on semicolons that are not inside quotes,
 * comments or PostgreSQL's `$tag$ … $tag$` blocks. Empty ones are left out.
 */
export function splitStatements(script: string): Statement[] {
    const statements: Statement[] = [];
    let start = 0;
    let at = 0;
    const push = (end: number) => {
        const raw = script.slice(start, end);
        const text = raw.trim();
        if (text) {
            const lead = raw.length - raw.trimStart().length;
            statements.push({ from: start + lead, to: start + lead + text.length, text });
        }
    };
    while (at < script.length) {
        const char = script[at];
        const next = script[at + 1];
        if (char === "-" && next === "-") {
            const end = script.indexOf("\n", at);
            at = end === -1 ? script.length : end + 1;
        } else if (char === "/" && next === "*") {
            const end = script.indexOf("*/", at + 2);
            at = end === -1 ? script.length : end + 2;
        } else if (char === "'" || char === '"' || char === "`") {
            at = closingQuote(script, at, char);
        } else if (char === "$") {
            const tag = /^\$[A-Za-z_]*\$/.exec(script.slice(at))?.[0];
            if (tag) {
                const end = script.indexOf(tag, at + tag.length);
                at = end === -1 ? script.length : end + tag.length;
            } else {
                at++;
            }
        } else if (char === ";") {
            push(at);
            start = at + 1;
            at++;
        } else {
            at++;
        }
    }
    push(script.length);
    return statements;
}

/** Past the quote that closes the one at `open`; a doubled quote or a backslash escapes it. */
function closingQuote(script: string, open: number, quote: string): number {
    let at = open + 1;
    while (at < script.length) {
        if (script[at] === "\\") {
            at += 2;
        } else if (script[at] === quote) {
            if (script[at + 1] === quote) at += 2;
            else return at + 1;
        } else {
            at++;
        }
    }
    return script.length;
}

/** The statement the cursor is in, or the one just before it when the cursor sits after a semicolon. */
export function statementAt(script: string, cursor: number): Statement | null {
    const statements = splitStatements(script);
    let before: Statement | null = null;
    for (const statement of statements) {
        if (cursor >= statement.from && cursor <= statement.to + 1) return statement;
        if (statement.to < cursor) before = statement;
    }
    return before ?? statements[0] ?? null;
}

/** A name written so the engine reads it as one identifier: backticks for MySQL, double quotes elsewhere. */
export function quoteIdentifier(engine: Engine, name: string): string {
    return engine === "mysql" ? `\`${name.replace(/`/g, "``")}\`` : `"${name.replace(/"/g, '""')}"`;
}

/** A query for a table's first rows, as a starting point to edit. */
export function previewSql(engine: Engine, schema: string, table: string, limit = 100): string {
    const qualified = schema ? `${quoteIdentifier(engine, schema)}.${quoteIdentifier(engine, table)}` : quoteIdentifier(engine, table);
    return `select * from ${qualified} limit ${limit};`;
}
