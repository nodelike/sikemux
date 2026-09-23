import { beforeEach, describe, expect, it, vi } from "vitest";
import { fsapi } from "../api/fs";
import { attachmentName, base64Of, imagesInClipboard, savePastedClipboard, savePastedImages } from "./pasteImage";

vi.mock("../api/fs", () => ({
    fsapi: { chatAttachmentDir: vi.fn(), saveBase64IntoDir: vi.fn(), clipboardPng: vi.fn() },
}));

/** A drop, or the rare paste that fills `files`. */
function clipboard(files: File[]): DataTransfer {
    return { files } as unknown as DataTransfer;
}

/** What WebKit actually hands a paste handler: items, and no files. */
function pastedItems(files: File[]): DataTransfer {
    return {
        files: [],
        items: files.map((file) => ({ kind: "file", type: file.type, getAsFile: () => file })),
    } as unknown as DataTransfer;
}

const png = (name = "image.png", type = "image/png") => new File([new Uint8Array([1, 2, 3])], name, { type });

describe("pasting a picture into the composer", () => {
    beforeEach(() => {
        vi.clearAllMocks();
        vi.mocked(fsapi.chatAttachmentDir).mockResolvedValue("/cache/pasted");
        vi.mocked(fsapi.saveBase64IntoDir).mockImplementation(async (dir, name) => `${dir}/${name}`);
        vi.mocked(fsapi.clipboardPng).mockResolvedValue(null);
    });

    it("takes the pictures out of a clipboard and leaves everything else", () => {
        const files = clipboard([png(), new File(["x"], "notes.txt", { type: "text/plain" })]);
        expect(imagesInClipboard(files).map((file) => file.name)).toEqual(["image.png"]);
    });

    it("finds nothing in an empty or absent clipboard", () => {
        expect(imagesInClipboard(null)).toEqual([]);
        expect(imagesInClipboard(clipboard([]))).toEqual([]);
    });

    it("finds a picture offered as an item with no files beside it", () => {
        expect(imagesInClipboard(pastedItems([png()])).map((file) => file.name)).toEqual(["image.png"]);
    });

    it("ignores a non-file item such as pasted text", () => {
        const data = { files: [], items: [{ kind: "string", type: "text/plain", getAsFile: () => null }] } as unknown as DataTransfer;
        expect(imagesInClipboard(data)).toEqual([]);
    });

    it("counts a picture once when it arrives as both an item and a file", () => {
        const file = png();
        const data = {
            files: [file],
            items: [{ kind: "file", type: file.type, getAsFile: () => file }],
        } as unknown as DataTransfer;

        expect(imagesInClipboard(data)).toHaveLength(1);
    });

    it("replaces the placeholder name a screenshot always arrives with", () => {
        const at = new Date(2026, 8, 19, 20, 5, 3, 42);
        expect(attachmentName(png(), at)).toBe("pasted-20260919-200503042.png");
        expect(attachmentName(png("image.jpg", "image/jpeg"), at)).toBe("pasted-20260919-200503042.jpg");
    });

    it("keeps a name the picture genuinely had", () => {
        expect(attachmentName(png("diagram.png"))).toBe("diagram.png");
    });

    it("encodes bytes without overflowing on a large picture", () => {
        expect(base64Of(new Uint8Array([104, 105]))).toBe("aGk=");
        expect(base64Of(new Uint8Array(200_000).fill(65))).toHaveLength(Math.ceil(200_000 / 3) * 4);
    });

    it("writes each picture into the scratch directory and reports the paths", async () => {
        const saved = await savePastedImages([png("diagram.png"), png("chart.png")]);

        expect(fsapi.chatAttachmentDir).toHaveBeenCalledTimes(1);
        expect(saved).toEqual(["/cache/pasted/diagram.png", "/cache/pasted/chart.png"]);
    });

    it("does not reach for a directory when there is nothing to save", async () => {
        expect(await savePastedImages([])).toEqual([]);
        expect(fsapi.chatAttachmentDir).not.toHaveBeenCalled();
    });

    it("asks AppKit when the paste event carries nothing", async () => {
        vi.mocked(fsapi.clipboardPng).mockResolvedValue("UE5H");

        const saved = await savePastedClipboard({ files: [], items: [] } as unknown as DataTransfer);

        expect(fsapi.clipboardPng).toHaveBeenCalledTimes(1);
        expect(saved).toHaveLength(1);
        expect(fsapi.saveBase64IntoDir).toHaveBeenCalledWith("/cache/pasted", expect.stringMatching(/^pasted-.*\.png$/), "UE5H");
    });

    it("does not ask AppKit when the event already had the picture", async () => {
        await savePastedClipboard(clipboard([png("diagram.png")]));

        expect(fsapi.clipboardPng).not.toHaveBeenCalled();
    });

    it("attaches nothing when neither the event nor AppKit has a picture", async () => {
        expect(await savePastedClipboard({ files: [], items: [] } as unknown as DataTransfer)).toEqual([]);
    });
});
