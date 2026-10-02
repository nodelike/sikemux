import { act, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { nativeViewsOccluded } from "../state/nativeViews";
import { TreeContextMenu } from "./FileTree";

describe("context menu over a browser page", () => {
    it("leaves the pages where they are while it is open", () => {
        render(<TreeContextMenu x={40} y={60} items={[{ label: "Close", run: () => {} }]} onClose={() => {}} />);
        expect(nativeViewsOccluded()).toBe(false);
    });

    it("closes when a click on the page takes the window's focus", () => {
        const onClose = vi.fn();
        render(<TreeContextMenu x={40} y={60} items={[{ label: "Close", run: () => {} }]} onClose={onClose} />);

        act(() => {
            window.dispatchEvent(new Event("blur"));
        });
        expect(onClose).toHaveBeenCalled();
    });
});
