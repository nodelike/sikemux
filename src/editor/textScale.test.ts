import { beforeEach, describe, expect, it } from "vitest";
import { applyEditorTextScale, clampEditorTextScale, DEFAULT_EDITOR_TEXT_SCALE } from "./textScale";

describe("editor text scale", () => {
    beforeEach(() => {
        document.documentElement.style.removeProperty("--editor-text-scale");
    });

    it("keeps the scale inside a readable range", () => {
        expect(clampEditorTextScale(0.2)).toBe(0.7);
        expect(clampEditorTextScale(9)).toBe(2);
        expect(clampEditorTextScale(Number.NaN)).toBe(DEFAULT_EDITOR_TEXT_SCALE);
    });

    it("rounds away the drift that repeated steps accumulate", () => {
        expect(clampEditorTextScale(1 + 0.1 + 0.1 + 0.1)).toBe(1.3);
    });

    it("publishes the scale as the variable the editor theme multiplies by", () => {
        applyEditorTextScale(1.4);

        expect(document.documentElement.style.getPropertyValue("--editor-text-scale")).toBe("1.4");
    });

    it("never publishes a value outside the range", () => {
        applyEditorTextScale(99);

        expect(document.documentElement.style.getPropertyValue("--editor-text-scale")).toBe("2");
    });
});
