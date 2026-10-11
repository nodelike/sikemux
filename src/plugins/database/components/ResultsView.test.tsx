import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ResultSet } from "../api";
import { ResultsView } from "./ResultsView";

const customers: ResultSet = {
    columns: [
        { name: "id", type: "int4", numeric: true },
        { name: "email", type: "text", numeric: false },
    ],
    rows: [
        [1, "ada@example.com"],
        [2, null],
    ],
    truncated: false,
    affected: null,
};

afterEach(cleanup);

describe("ResultsView", () => {
    it("marks only the picked cell as selected", () => {
        render(<ResultsView outcome={{ results: [customers], millis: 1 }} />);
        const email = screen.getByRole("cell", { name: "ada@example.com" });
        const missing = screen.getByRole("cell", { name: "NULL" });
        fireEvent.click(email);
        expect(email).toHaveAttribute("aria-selected", "true");
        fireEvent.click(missing);
        expect(email).toHaveAttribute("aria-selected", "false");
        expect(missing).toHaveClass("selected");
        expect(screen.getByLabelText("Selected cell")).toHaveTextContent("email");
        const [rowNumber, id] = screen.getAllByRole("cell", { name: "2" });
        fireEvent.click(rowNumber);
        expect(missing).toHaveClass("selected");
        fireEvent.click(id);
        expect(missing).not.toHaveClass("selected");
        expect(id).toHaveClass("selected");
    });

    it("draws a long result a page at a time", () => {
        const many: ResultSet = { ...customers, rows: Array.from({ length: 1500 }, (_, at) => [at, `row ${at}`]) };
        render(<ResultsView outcome={{ results: [many], millis: 1 }} />);
        expect(screen.queryByText("row 1200")).toBeNull();
        fireEvent.click(screen.getByRole("button", { name: "Show 500 more rows" }));
        expect(screen.getByText("row 1200")).toBeInTheDocument();
    });
});
