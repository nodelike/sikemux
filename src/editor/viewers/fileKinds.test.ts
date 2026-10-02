import { describe, expect, it } from "vitest";
import { extname, isImagePath, isPreviewPath, viewerKind } from "./fileKinds";

describe("file kinds", () => {
    it("extracts lowercase extensions", () => {
        expect(extname("/tmp/photo.PNG")).toBe("png");
        expect(extname("/tmp/archive.tar.gz")).toBe("gz");
        expect(extname(".gitignore")).toBe("");
        expect(extname("noext")).toBe("");
    });

    it("detects image paths", () => {
        expect(isImagePath("diagram.svg")).toBe(true);
        expect(isImagePath("photo.HEIC")).toBe(true);
        expect(isImagePath("README.md")).toBe(false);
        expect(isImagePath(null)).toBe(false);
    });

    it("sends files that are never text straight to a viewer", () => {
        expect(isPreviewPath("/a/report.pdf")).toBe(true);
        expect(isPreviewPath("/a/deck.pptx")).toBe(true);
        expect(isPreviewPath("/a/server.key")).toBe(false);
        expect(isPreviewPath("/a/main.ts")).toBe(false);
    });

    it("picks a viewer from the type the backend reports", () => {
        expect(viewerKind("image/heic")).toBe("image");
        expect(viewerKind("image/vnd.adobe.photoshop")).toBe("document");
        expect(viewerKind("application/pdf")).toBe("pdf");
        expect(viewerKind("video/quicktime")).toBe("video");
        expect(viewerKind("video/x-matroska")).toBe("binary");
        expect(viewerKind("audio/mp4")).toBe("audio");
        expect(viewerKind("font/woff2")).toBe("font");
        expect(viewerKind("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")).toBe("document");
        expect(viewerKind("application/zip")).toBe("binary");
    });
});
