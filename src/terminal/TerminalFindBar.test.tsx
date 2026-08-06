import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalFindBar } from "./TerminalFindBar";
import type { TerminalController } from "./useXterm";

function controller(): TerminalController {
    return {
        find: vi.fn(() => true),
        clearSearch: vi.fn(),
        getSelection: vi.fn(() => ""),
        copySelection: vi.fn(async () => false),
        pasteClipboard: vi.fn(async () => false),
        selectAll: vi.fn(),
        copyScrollback: vi.fn(async () => false),
        clear: vi.fn(),
        focus: vi.fn(),
    };
}

afterEach(cleanup);

describe("TerminalFindBar", () => {
    it("searches incrementally and navigates in both directions", () => {
        const ctl = controller();
        const onQueryChange = vi.fn();
        const options = { caseSensitive: false, regex: false, wholeWord: false };

        render(
            <TerminalFindBar
                controller={ctl}
                query="ready"
                onQueryChange={onQueryChange}
                options={options}
                onOptionsChange={vi.fn()}
                result={{ resultIndex: 1, resultCount: 4 }}
                onClose={vi.fn()}
            />,
        );

        expect(ctl.find).toHaveBeenCalledWith("ready", "next", options, true);
        expect(screen.getByText("2/4")).toBeInTheDocument();
        fireEvent.keyDown(screen.getByLabelText("Find in terminal"), { key: "Enter", shiftKey: true });
        expect(ctl.find).toHaveBeenLastCalledWith("ready", "previous", options);
        fireEvent.click(screen.getByLabelText("Next match"));
        expect(ctl.find).toHaveBeenLastCalledWith("ready", "next", options);
    });

    it("updates options and closes on Escape", () => {
        const ctl = controller();
        const onOptionsChange = vi.fn();
        const onClose = vi.fn();
        const options = { caseSensitive: false, regex: false, wholeWord: false };
        render(
            <TerminalFindBar
                controller={ctl}
                query=""
                onQueryChange={vi.fn()}
                options={options}
                onOptionsChange={onOptionsChange}
                result={{ resultIndex: -1, resultCount: 0 }}
                onClose={onClose}
            />,
        );

        fireEvent.click(screen.getByTitle("Use regular expression"));
        expect(onOptionsChange).toHaveBeenCalledWith({ ...options, regex: true });
        fireEvent.keyDown(screen.getByLabelText("Find in terminal"), { key: "Escape" });
        expect(onClose).toHaveBeenCalledOnce();
    });
});
