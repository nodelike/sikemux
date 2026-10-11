import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorSelection, EditorState, EditorView } from "../../../plugin-api/editor";
import { SqlEditor, sqlUnderCursor } from "./SqlEditor";

afterEach(cleanup);

function viewOf(doc: string, anchor: number, head = anchor): EditorView {
    return new EditorView({ state: EditorState.create({ doc, selection: EditorSelection.single(anchor, head) }) });
}

describe("SqlEditor", () => {
    it("runs the statement under the cursor when nothing is selected", () => {
        const doc = "select 1;\nselect 2;";
        expect(sqlUnderCursor(viewOf(doc, 3))).toBe("select 1");
        expect(sqlUnderCursor(viewOf(doc, doc.length))).toBe("select 2");
    });

    it("runs exactly the selection when there is one", () => {
        expect(sqlUnderCursor(viewOf("select 1; select 2;", 10, 18))).toBe("select 2");
    });

    it("shows the SQL it is given and follows changes from outside", () => {
        const { rerender } = render(<SqlEditor value="select 1" dialect="postgres" onChange={vi.fn()} onRun={vi.fn()} />);
        expect(screen.getByLabelText("SQL")).toHaveTextContent("select 1");
        rerender(<SqlEditor value="select * from orders" dialect="postgres" onChange={vi.fn()} onRun={vi.fn()} />);
        expect(screen.getByLabelText("SQL")).toHaveTextContent("select * from orders");
    });
});
