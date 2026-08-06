import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalContextMenu } from "./TerminalContextMenu";
import type { TerminalController } from "./useXterm";

function controller(selection: string): TerminalController {
    return {
        find: vi.fn(() => true),
        clearSearch: vi.fn(),
        getSelection: vi.fn(() => selection),
        copySelection: vi.fn(async () => true),
        pasteClipboard: vi.fn(async () => true),
        selectAll: vi.fn(),
        copyScrollback: vi.fn(async () => true),
        clear: vi.fn(),
        focus: vi.fn(),
    };
}

afterEach(cleanup);

describe("TerminalContextMenu", () => {
    it("uses the current selection for copy and find", () => {
        const copyCtl = controller("selected output");
        const closeCopy = vi.fn();
        const first = render(<TerminalContextMenu x={20} y={20} controller={copyCtl} onFind={vi.fn()} onClose={closeCopy} />);
        fireEvent.click(screen.getByRole("menuitem", { name: /Copy(?:⌘|Ctrl\+)C/ }));
        expect(copyCtl.copySelection).toHaveBeenCalledOnce();
        expect(closeCopy).toHaveBeenCalledOnce();
        first.unmount();

        const findCtl = controller("selected output");
        const onFind = vi.fn();
        render(<TerminalContextMenu x={20} y={20} controller={findCtl} onFind={onFind} onClose={vi.fn()} />);
        fireEvent.click(screen.getByRole("menuitem", { name: /Find(?:⌘|Ctrl\+)F/ }));
        expect(onFind).toHaveBeenCalledWith("selected output");
    });

    it("disables copy when there is no selection", () => {
        render(<TerminalContextMenu x={20} y={20} controller={controller("")} onFind={vi.fn()} onClose={vi.fn()} />);
        expect(screen.getByRole("menuitem", { name: /Copy(?:⌘|Ctrl\+)C/ })).toBeDisabled();
    });
});
