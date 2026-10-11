import { afterEach, describe, expect, it, vi } from "vitest";
import { fsapi } from "../api/fs";
import { localImagePath, localPath, previewCacheBytes, readImageSource, sizedSvg, useImagePreview } from "./imagePreview";
import { act, renderHook, waitFor } from "@testing-library/react";

const url = (path: string) => `preview://localhost/${encodeURIComponent(path)}`;

/* Stands in for the backend: `previewFile` reports on a file and `fetch` reads it from the preview scheme. */
function serve(files: Record<string, { mime: string; size?: number; body?: string }>) {
    const preview = vi.spyOn(fsapi, "previewFile").mockImplementation(async (path) => {
        const file = files[path];
        if (!file) throw new Error("unreadable");
        return { mime: file.mime, size: file.size ?? file.body?.length ?? 0, modified: 0 };
    });
    vi.stubGlobal(
        "fetch",
        vi.fn(async (href: string) => new Response(files[decodeURIComponent(new URL(href).pathname.slice(1))]?.body ?? "")),
    );
    return preview;
}

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

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
        serve(Object.fromEntries(Array.from({ length: 20 }, (_, index) => [`/shots/${index}.png`, { mime: "image/png", size: 1024 }])));

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
        serve({ "/shots/retina.png": { mime: "image/png", size: 4_634_596, body: "PNG" } });
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
        const read = serve({ "/shots/unshrinkable.png": { mime: "image/png", size: 4_634_596, body: "PNG" } });
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
        serve({ "/Downloads/codex-icon.svg": { mime: "image/svg+xml", body: markup } });

        const { result } = renderHook(() => useImagePreview("/Downloads/codex-icon.svg"));

        await waitFor(() => expect(result.current).not.toBeNull());
        const drawn = decodeURIComponent(result.current!.replace("data:image/svg+xml;charset=utf-8,", ""));
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

    it("has no local path for an empty, relative or undecodable reference", () => {
        expect(localPath(null)).toBeNull();
        expect(localPath("")).toBeNull();
        expect(localPath("shots/a.png")).toBeNull();
        expect(localPath("file:///tmp/%E0%A4%A.png")).toBeNull();
    });

    it("leaves an SVG alone when its viewBox names no usable size or it is not an SVG", () => {
        const flat = '<svg viewBox="0 0 0 10" xmlns="http://www.w3.org/2000/svg"/>';
        const bare = '<svg xmlns="http://www.w3.org/2000/svg"/>';
        const other = "<html/>";

        expect(sizedSvg(flat)).toBe(flat);
        expect(sizedSvg(bare)).toBe(bare);
        expect(sizedSvg(other)).toBe(other);
    });

    it("shows a whole image from disk and nothing for a file that is not one", async () => {
        serve({ "/shots/whole.png": { mime: "image/png", size: 4_000_000 }, "/shots/actually-text.png": { mime: "text/plain" } });

        expect(await readImageSource("/shots/whole.png")).toBe(url("/shots/whole.png"));
        expect(await readImageSource("/shots/actually-text.png")).toBeNull();
        expect(await readImageSource("/shots/missing.png")).toBeNull();
    });

    it("shows nothing for no path or a path that is not an image, without reading it", () => {
        const read = vi.spyOn(fsapi, "previewFile");
        const { result, rerender } = renderHook(({ path }: { path: string | null }) => useImagePreview(path), {
            initialProps: { path: null as string | null },
        });

        expect(result.current).toBeNull();
        rerender({ path: "/notes/readme.md" });
        expect(result.current).toBeNull();
        expect(read).not.toHaveBeenCalled();
    });

    it("reads a picture once for every view of it, and remembers one it could not read", async () => {
        const read = serve({ "/shots/shared.png": { mime: "image/png", size: 3 } });

        const first = renderHook(() => useImagePreview("/shots/shared.png"));
        const second = renderHook(() => useImagePreview("/shots/shared.png"));
        await waitFor(() => expect(first.result.current).toBe(url("/shots/shared.png")));
        await waitFor(() => expect(second.result.current).toBe(url("/shots/shared.png")));
        const later = renderHook(() => useImagePreview("/shots/shared.png"));
        expect(later.result.current).toBe(url("/shots/shared.png"));

        const broken = renderHook(() => useImagePreview("/shots/broken.png"));
        await waitFor(() => expect(read).toHaveBeenCalledWith("/shots/broken.png"));
        await act(async () => {});
        renderHook(() => useImagePreview("/shots/broken.png"));

        expect(broken.result.current).toBeNull();
        expect(read.mock.calls.map(([path]) => path)).toEqual(["/shots/shared.png", "/shots/broken.png"]);
    });

    it("has no preview for a big picture the webview fails to decode or has no canvas for", async () => {
        serve({
            "/shots/undecodable.png": { mime: "image/png", size: 4_000_000, body: "PNG" },
            "/shots/canvasless.png": { mime: "image/png", size: 4_000_000, body: "PNG" },
        });
        const decode = vi.fn(async () => {
            throw new Error("undecodable");
        });
        vi.stubGlobal("createImageBitmap", decode);
        const undecodable = renderHook(() => useImagePreview("/shots/undecodable.png"));
        await waitFor(() => expect(decode).toHaveBeenCalled());
        await act(async () => {});

        const close = vi.fn();
        vi.stubGlobal("createImageBitmap", async () => ({ width: 10, height: 10, close }));
        vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
        const canvasless = renderHook(() => useImagePreview("/shots/canvasless.png"));
        await waitFor(() => expect(close).toHaveBeenCalled());
        await act(async () => {});

        expect(undecodable.result.current).toBeNull();
        expect(canvasless.result.current).toBeNull();
    });

    it("shows no preview for a file named like a picture that is not one", async () => {
        const read = serve({ "/shots/actually-html.png": { mime: "text/html" } });
        const { result } = renderHook(() => useImagePreview("/shots/actually-html.png"));

        await waitFor(() => expect(read).toHaveBeenCalled());
        await act(async () => {});
        expect(result.current).toBeNull();
    });

    it("keeps a preview that arrives after its view moved on to another file out of that view", async () => {
        let answer: (preview: { mime: string; size: number; modified: number }) => void = () => {};
        vi.spyOn(fsapi, "previewFile").mockImplementation((path) =>
            path === "/shots/slow.png" ? new Promise((resolve) => (answer = resolve)) : Promise.resolve({ mime: "image/png", size: 4, modified: 0 }),
        );
        const { result, rerender } = renderHook(({ path }) => useImagePreview(path), { initialProps: { path: "/shots/slow.png" } });
        rerender({ path: "/shots/fast.png" });
        await waitFor(() => expect(result.current).toBe(url("/shots/fast.png")));

        await act(async () => answer({ mime: "image/png", size: 4, modified: 0 }));

        expect(result.current).toBe(url("/shots/fast.png"));
    });

    /* The transcript mounts a row again as it scrolls back; a picture that
       arrived a render late would grow the row and move everything below it. */
    it("draws a preview it already holds on the first render", async () => {
        serve({ "/shots/again.png": { mime: "image/png", size: 4 } });
        const first = renderHook(() => useImagePreview("/shots/again.png"));
        await waitFor(() => expect(first.result.current).toBe(url("/shots/again.png")));
        first.unmount();

        const rendered: (string | null)[] = [];
        renderHook(() => {
            const preview = useImagePreview("/shots/again.png");
            rendered.push(preview);
            return preview;
        });

        expect(rendered[0]).toBe(url("/shots/again.png"));
    });
});
