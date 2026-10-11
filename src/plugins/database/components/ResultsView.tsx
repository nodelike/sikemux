import { memo, useState, type MouseEvent, type ReactNode } from "react";
import { copyText, swallow } from "../../../plugin-api/host";
import type { Cell, QueryOutcome, ResultColumn, ResultSet } from "../api";
import { cellText, duration, mainResult, summary, toCsv, toTsv } from "../results";

/** Rows drawn at a time; more are added on request so a 10,000-row result stays quick to show. */
const PAGE = 1000;

export function ResultsView({ outcome, actions }: { outcome: QueryOutcome; actions?: (result: ResultSet) => ReactNode }) {
    const [picked, setPicked] = useState(() => mainResult(outcome));
    const index = Math.min(picked, outcome.results.length - 1);
    const result = outcome.results[index];
    if (!result) return <div className="db-results-empty">The statement ran and returned nothing.</div>;

    return (
        <section className="db-results" aria-label="Results">
            <div className="db-results-bar">
                {outcome.results.length > 1 && (
                    <div className="db-result-tabs" role="tablist" aria-label="Statements">
                        {outcome.results.map((each, at) => (
                            <button
                                key={at}
                                type="button"
                                role="tab"
                                aria-selected={at === index}
                                className={at === index ? "active" : undefined}
                                title={summary(each)}
                                onClick={() => setPicked(at)}>
                                {at + 1}
                            </button>
                        ))}
                    </div>
                )}
                <span className="db-meta" role="status">
                    {summary(result)} · {duration(outcome.millis)}
                </span>
                <span className="db-grow" />
                {result.columns.length > 0 && (
                    <button
                        type="button"
                        className="db-button"
                        title="Copy as tab-separated text, ready to paste into a spreadsheet"
                        onClick={() => void copyText(toTsv(result)).catch(swallow("copy the results"))}>
                        Copy
                    </button>
                )}
                {result.columns.length > 0 && (
                    <button
                        type="button"
                        className="db-button"
                        title="Copy as comma-separated values (CSV)"
                        onClick={() => void copyText(toCsv(result)).catch(swallow("copy the results"))}>
                        Copy CSV
                    </button>
                )}
                {actions?.(result)}
            </div>
            {result.columns.length > 0 ? <Grid key={index} result={result} /> : <div className="db-results-empty">{summary(result)}.</div>}
        </section>
    );
}

function Grid({ result }: { result: ResultSet }) {
    const [shown, setShown] = useState(PAGE);
    const [selected, setSelected] = useState<{ row: number; column: number } | null>(null);
    const selectedCell: Cell | undefined = selected ? result.rows[selected.row]?.[selected.column] : undefined;
    const pick = (event: MouseEvent<HTMLElement>) => {
        const clicked = clickedCell(event);
        if (clicked) setSelected(clicked);
    };

    return (
        <>
            <div className="db-grid-scroll">
                <table className="db-grid">
                    <thead>
                        <tr>
                            <th className="db-rownum" aria-label="Row" />
                            {result.columns.map((column, at) => (
                                <th key={at} className={column.numeric ? "numeric" : undefined} title={column.type || undefined}>
                                    <span className="db-col-name">{column.name}</span>
                                    {column.type && <span className="db-col-type">{column.type}</span>}
                                </th>
                            ))}
                        </tr>
                    </thead>
                    <tbody onClick={pick}>
                        {result.rows.slice(0, shown).map((row, rowAt) => (
                            <GridRow
                                key={rowAt}
                                row={row}
                                at={rowAt}
                                columns={result.columns}
                                selectedColumn={selected?.row === rowAt ? selected.column : null}
                            />
                        ))}
                    </tbody>
                </table>
                {result.rows.length > shown && (
                    <button type="button" className="db-button db-show-more" onClick={() => setShown((count) => count + PAGE)}>
                        Show {Math.min(PAGE, result.rows.length - shown).toLocaleString("en-US")} more rows
                    </button>
                )}
            </div>
            {selected && selectedCell !== undefined && (
                <div className="db-cell-inspector" aria-label="Selected cell">
                    <span className="db-meta">{result.columns[selected.column]?.name}</span>
                    <code className={selectedCell === null ? "null" : undefined}>{cellText(selectedCell)}</code>
                    <button
                        type="button"
                        className="db-button"
                        onClick={() => void copyText(selectedCell === null ? "" : cellText(selectedCell)).catch(swallow("copy the value"))}>
                        Copy value
                    </button>
                </div>
            )}
        </>
    );
}

/** The cell a click in the grid's body landed on, read from the row and column it carries. */
function clickedCell(event: MouseEvent<HTMLElement>): { row: number; column: number } | null {
    const cell = (event.target as HTMLElement).closest<HTMLElement>("td[data-column]");
    const row = cell?.parentElement?.dataset.row;
    if (!cell || row === undefined) return null;
    return { row: Number(row), column: Number(cell.dataset.column) };
}

/** One row, drawn again only when its cells or which of them is selected change. */
const GridRow = memo(function GridRow({
    row,
    at,
    columns,
    selectedColumn,
}: {
    row: Cell[];
    at: number;
    columns: ResultColumn[];
    selectedColumn: number | null;
}) {
    return (
        <tr data-row={at}>
            <td className="db-rownum">{at + 1}</td>
            {row.map((cell, columnAt) => {
                const isSelected = selectedColumn === columnAt;
                const classes = [columns[columnAt]?.numeric ? "numeric" : "", cell === null ? "null" : "", isSelected ? "selected" : ""]
                    .filter(Boolean)
                    .join(" ");
                return (
                    <td key={columnAt} data-column={columnAt} className={classes || undefined} aria-selected={isSelected}>
                        {cellText(cell)}
                    </td>
                );
            })}
        </tr>
    );
});
