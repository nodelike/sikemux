import { codeFence } from "../../plugin-api/ui";
import type { Cell, QueryOutcome, ResultSet } from "./api";

/** What a cell shows; null is told apart from empty text by the grid's styling, not by this text. */
export function cellText(cell: Cell): string {
    if (cell === null) return "NULL";
    if (typeof cell === "boolean") return cell ? "true" : "false";
    return String(cell);
}

const plural = (count: number, one: string, many: string) => `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`;

/** One line about a result: how many rows it returned or changed, and whether some were left out. */
export function summary(result: ResultSet): string {
    if (result.affected !== null && result.columns.length === 0) return `${plural(result.affected, "row", "rows")} changed`;
    const rows = plural(result.rows.length, "row", "rows");
    return result.truncated ? `First ${rows}; more were left out` : rows;
}

/** The result shown when a run gives several: the last one that returned rows, or else the last of all. */
export function mainResult(outcome: QueryOutcome): number {
    for (let index = outcome.results.length - 1; index >= 0; index--) {
        if (outcome.results[index].columns.length > 0) return index;
    }
    return Math.max(outcome.results.length - 1, 0);
}

const tsvField = (cell: Cell) => (cell === null ? "" : cellText(cell).replace(/[\t\n\r]+/g, " "));

/** Tab-separated, with a header row, the way spreadsheets paste it. */
export function toTsv(result: ResultSet): string {
    const header = result.columns.map((column) => column.name).join("\t");
    return [header, ...result.rows.map((row) => row.map(tsvField).join("\t"))].join("\n");
}

const csvField = (cell: Cell) => {
    if (cell === null) return "";
    const text = cellText(cell);
    return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
};

/** Comma-separated with a header row; fields with commas, quotes or line breaks are quoted. NULL is an empty field. */
export function toCsv(result: ResultSet): string {
    const header = result.columns.map((column) => csvField(column.name)).join(",");
    return [header, ...result.rows.map((row) => row.map(csvField).join(","))].join("\r\n");
}

const markdownField = (cell: Cell) =>
    cellText(cell)
        .replace(/\|/g, "\\|")
        .replace(/[\r\n]+/g, " ");

/** A markdown table for an agent, cut to `maxRows` so a big result does not flood its context. */
export function toMarkdown(result: ResultSet, maxRows = 50): string {
    if (result.columns.length === 0) return summary(result);
    const header = `| ${result.columns.map((column) => markdownField(column.name)).join(" | ")} |`;
    const rule = `| ${result.columns.map((column) => (column.numeric ? "---:" : "---")).join(" | ")} |`;
    const rows = result.rows.slice(0, maxRows).map((row) => `| ${row.map(markdownField).join(" | ")} |`);
    const left = result.rows.length - rows.length;
    const note =
        left > 0 || result.truncated ? `\n\n${summary(result)}${left > 0 ? `; ${plural(left, "more row", "more rows")} not shown` : ""}` : "";
    return [header, rule, ...rows].join("\n") + note;
}

/** What an agent is handed: which database, the SQL that ran, and its results as a table. */
export function resultMessage(database: string, engine: string, sql: string, result: ResultSet): string {
    return `Results of a query on the ${engine} database "${database}":\n\n${codeFence(sql, "sql")}\n\n${toMarkdown(result)}`;
}
