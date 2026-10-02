import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { documentPreviewApi } from "../../api/documentPreview";
import { occludeNativeViews } from "../../state/nativeViews";
import { DocumentView } from "./DocumentView";

const preview = { mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation", size: 1024, modified: 1 };

describe("DocumentView", () => {
    beforeEach(() => {
        vi.spyOn(documentPreviewApi, "show").mockResolvedValue();
        vi.spyOn(documentPreviewApi, "hide").mockResolvedValue();
        vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(240.4, 70.6, 800, 600));
    });

    afterEach(() => {
        cleanup();
        vi.restoreAllMocks();
    });

    const view = (visible = true) => (
        <DocumentView path="/repo/deck.pptx" preview={preview} url="preview://localhost/deck" onReload={() => {}} visible={visible} />
    );

    it("lays the native view over its stage in whole pixels", () => {
        render(view());
        expect(documentPreviewApi.show).toHaveBeenLastCalledWith(expect.any(String), "/repo/deck.pptx", { x: 240, y: 71, width: 800, height: 600 });
    });

    it("steps aside for an overlay and comes back after it", () => {
        render(view());
        let release = () => {};
        act(() => {
            release = occludeNativeViews();
        });
        expect(documentPreviewApi.hide).toHaveBeenCalled();
        vi.mocked(documentPreviewApi.show).mockClear();
        act(() => release());
        expect(documentPreviewApi.show).toHaveBeenCalledTimes(1);
    });

    it("hides while its pane is out of sight and when it closes", () => {
        const { rerender, unmount } = render(view());
        const owner = vi.mocked(documentPreviewApi.show).mock.calls[0][0];
        rerender(view(false));
        expect(documentPreviewApi.hide).toHaveBeenLastCalledWith(owner);
        vi.mocked(documentPreviewApi.hide).mockClear();
        unmount();
        expect(documentPreviewApi.hide).toHaveBeenCalledWith(owner);
    });
});
