import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FileIcon } from "./FileIcon";

describe("FileIcon", () => {
    it("renders special file and extension glyphs", () => {
        const { container, rerender } = render(<FileIcon name="package.json" size={20} />);
        const first = container.querySelector(".file-glyph") as HTMLElement;
        expect(first).toHaveTextContent("");
        expect(first.style.fontSize).toBe("20px");

        rerender(<FileIcon name="App.tsx" />);
        expect(container.querySelector(".file-glyph")).toHaveTextContent("");

        rerender(<FileIcon name="index.astro" />);
        expect(container.querySelector(".file-glyph")).toHaveTextContent("");

        rerender(<FileIcon name="Main.kt" />);
        expect(container.querySelector(".file-glyph")).toHaveTextContent("");

        rerender(<FileIcon name="Justfile" />);
        expect(container.querySelector(".file-glyph")).toHaveTextContent("");

        rerender(<FileIcon name="unknownfile" />);
        expect(container.querySelector(".file-glyph")).toHaveTextContent("");
    });
});
