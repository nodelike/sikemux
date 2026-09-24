import { describe, expect, it, vi } from "vitest";
import { fsapi } from "../api/fs";
import { localImagePath, localPath, previewCacheBytes, sizedSvg, useImagePreview } from "./imagePreview";
import { act, renderHook, waitFor } from "@testing-library/react";

describe("chat image previews", () => {
    it("decodes the file URL an agent writes for an attachment", () => {
        expect(localImagePath("file:///Users/me/Screenshots/Shot%202026-09-16%20at%205.31.48%E2%80%AFPM.png")).toBe(
            "/Users/me/Screenshots/Shot 2026-09-16 at 5.31.48\u202fPM.png",
        );
    });

    it("takes a plain absolute path as it is", () => {
        expect(localImagePath("/tmp/diagram.jpeg")).toBe("/tmp/diagram.jpeg");
    });

    it("drops the leading slash a Windows file URL carries", () => {
        expect(localImagePath("file:///C:/Users/me/shot.png")).toBe("C:/Users/me/shot.png");
    });

    it("has no preview for files that are not images", () => {
        expect(localImagePath("file:///Users/me/notes.md")).toBeNull();
        expect(localPath("file:///Users/me/notes.md")).toBe("/Users/me/notes.md");
    });

    it("holds only a handful of thumbnails at a time", async () => {
        const megabyte = "A".repeat(1024 * 1024);
        vi.spyOn(fsapi, "readFileBase64").mockResolvedValue({ mime: "image/png", data: megabyte, size: 1024 });

        for (let index = 0; index < 20; index += 1) {
            const { unmount } = renderHook(() => useImagePreview(`/shots/${index}.png`));
            await waitFor(() => expect(previewCacheBytes()).toBeGreaterThan(0));
            act(() => unmount());
        }

        expect(previewCacheBytes()).toBeLessThanOrEqual(12 * 1024 * 1024);
        vi.restoreAllMocks();
    });

    /* A retina screenshot is several megabytes, which used to be refused
       outright and left the composer showing a file icon. */
    it("shrinks a screenshot rather than refusing to preview it", async () => {
        const thumb = "data:image/jpeg;base64,VEhVTUI=";
        vi.spyOn(fsapi, "readFileBase64").mockResolvedValue({ mime: "image/png", data: "A".repeat(1024 * 1024), size: 4_634_596 });
        vi.stubGlobal("createImageBitmap", async () => ({ width: 3024, height: 1890, close: () => {} }));
        vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
        const drawn = vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue(thumb);

        const { result } = renderHook(() => useImagePreview("/shots/retina.png"));

        await waitFor(() => expect(result.current).toBe(thumb));
        expect(drawn.mock.instances[0]).toMatchObject({ width: 720, height: 450 });
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    /* A window with no canvas to draw on has nothing to shrink a picture with,
       and a file icon beats parking six megabytes in the cache. */
    it("has no preview for a big picture it cannot shrink", async () => {
        const read = vi.spyOn(fsapi, "readFileBase64").mockResolvedValue({ mime: "image/png", data: "A".repeat(1024 * 1024), size: 4_634_596 });
        vi.stubGlobal("createImageBitmap", undefined);
        const held = previewCacheBytes();

        const { result } = renderHook(() => useImagePreview("/shots/unshrinkable.png"));

        await waitFor(() => expect(read).toHaveBeenCalled());
        expect(result.current).toBeNull();
        expect(previewCacheBytes()).toBeLessThanOrEqual(held);
        vi.unstubAllGlobals();
        vi.restoreAllMocks();
    });

    it("gives an SVG that only has a viewBox the size its viewBox names", async () => {
        const markup = '<svg viewBox="0 0 512 512" xmlns="http://www.w3.org/2000/svg"><path d="M0 0h16v16H0z"/></svg>';
        vi.spyOn(fsapi, "readFileBase64").mockResolvedValue({ mime: "image/svg+xml", data: btoa(markup), size: markup.length });

        const { result } = renderHook(() => useImagePreview("/Downloads/codex-icon.svg"));

        await waitFor(() => expect(result.current).not.toBeNull());
        const drawn = atob(result.current!.replace("data:image/svg+xml;base64,", ""));
        expect(drawn).toContain('width="512"');
        expect(drawn).toContain('height="512"');
        vi.restoreAllMocks();
    });

    it("leaves an SVG that already has a size as it is", () => {
        const markup = '<svg width="24" height="24" viewBox="0 0 512 512" xmlns="http://www.w3.org/2000/svg"/>';
        expect(sizedSvg(markup)).toBe(markup);
    });

    it("has no local path for remote links", () => {
        expect(localPath("https://example.com/cat.png")).toBeNull();
        expect(localImagePath("https://example.com/cat.png")).toBeNull();
    });
});
