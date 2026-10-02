import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { lsp } from "../api/lsp";
import { DiagnosticsController } from "../workbench/diagnosticsController";
import { EditorInsights } from "./EditorInsights";

vi.mock("../api/agents", () => ({ agentApi: { available: vi.fn(async () => []) } }));

const PROJECT = "/repo";
const PATH = "/repo/src/app.ts";

function controllerWithProblem(): DiagnosticsController {
    const controller = new DiagnosticsController(PROJECT);
    controller.activateServer("typescript", 1);
    controller.publish(
        {
            project: PROJECT,
            language: "typescript",
            path: PATH,
            version: 2,
            diagnostics: [
                {
                    range: { start: { line: 3, character: 4 }, end: { line: 3, character: 8 } },
                    severity: "error",
                    code: "TS100",
                    source: "typescript",
                    message: "Expected a value",
                },
            ],
        },
        1,
    );
    return controller;
}

describe("EditorInsights", () => {
    beforeEach(() => vi.restoreAllMocks());
    afterEach(cleanup);

    it("keeps diagnostics live while collapsed and navigates from Problems", () => {
        const controller = controllerWithProblem();
        const onNavigate = vi.fn();
        render(<EditorInsights project={PROJECT} path={PATH} controller={controller} visible onNavigate={onNavigate} />);

        expect(screen.getByRole("button", { name: "Problems 1" })).toHaveAttribute("aria-expanded", "false");
        expect(screen.queryByText("Expected a value")).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Problems 1" }));
        fireEvent.click(screen.getByRole("button", { name: /Expected a value/ }));
        expect(onNavigate).toHaveBeenCalledWith(PATH, 3, 4);
        expect(screen.getByText("src/app.ts:4:5")).toBeInTheDocument();
    });

    it("offers to send a problem to an agent from its row's menu", () => {
        render(<EditorInsights project={PROJECT} path={PATH} controller={controllerWithProblem()} visible onNavigate={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Problems 1" }));
        fireEvent.contextMenu(screen.getByRole("button", { name: /Expected a value/ }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Send to Agent…" }));
        expect(screen.getByRole("menu")).toBeInTheDocument();
        expect(screen.queryByRole("menuitem", { name: "Go to Problem" })).not.toBeInTheDocument();
    });

    it("loads a hierarchical outline only after the tab opens", async () => {
        const symbols = vi.spyOn(lsp, "documentSymbols").mockResolvedValueOnce([
            {
                name: "App",
                detail: "class App",
                kind: 5,
                range: { start: { line: 1, character: 0 }, end: { line: 8, character: 0 } },
                selectionRange: { start: { line: 1, character: 6 }, end: { line: 1, character: 9 } },
                children: [
                    {
                        name: "render",
                        detail: null,
                        kind: 6,
                        range: { start: { line: 2, character: 4 }, end: { line: 6, character: 4 } },
                        selectionRange: { start: { line: 2, character: 4 }, end: { line: 2, character: 10 } },
                        children: [],
                    },
                ],
            },
        ]);
        const onNavigate = vi.fn();
        const view = render(<EditorInsights project={PROJECT} path={PATH} controller={null} visible onNavigate={onNavigate} />);
        expect(symbols).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "Outline" }));
        await waitFor(() => expect(screen.getByTitle("class App")).toBeInTheDocument());
        expect(symbols).toHaveBeenCalledWith(PROJECT, "typescript", PATH);

        fireEvent.click(screen.getByTitle("render"));
        expect(onNavigate).toHaveBeenCalledWith(PATH, 2, 4);

        symbols.mockReturnValueOnce(new Promise(() => {}));
        view.rerender(<EditorInsights project={PROJECT} path="/repo/src/other.ts" controller={null} visible onNavigate={onNavigate} />);
        expect(screen.queryByTitle("class App")).not.toBeInTheDocument();
        expect(screen.getByText("Loading outline…")).toBeInTheDocument();
    });

    it("does not request symbols while the editor is hidden", () => {
        const symbols = vi.spyOn(lsp, "documentSymbols").mockResolvedValue([]);
        render(<EditorInsights project={PROJECT} path={PATH} controller={null} visible={false} onNavigate={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Outline" }));
        expect(symbols).not.toHaveBeenCalled();
    });
});
