import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ImageViewer } from "./ImageViewer";
import { hideImage, showImage } from "../state/imageViewer";
import { useToasts } from "../state/toast";

vi.mock("../api/fs", async () => {
    const actual = await vi.importActual<typeof import("../api/fs")>("../api/fs");
    return {
        ...actual,
        fsapi: {
            ...actual.fsapi,
            previewFile: vi.fn(),
            downloadsDir: vi.fn(),
            copyIntoDir: vi.fn(),
            saveBase64IntoDir: vi.fn(),
            revealInFinder: vi.fn(),
        },
    };
});

const { fsapi } = await import("../api/fs");

const shot = { src: "data:image/png;base64,SEVMTE8=", name: "shot.png", path: "/repo/shot.png" };
const fullShot = `preview://localhost/${encodeURIComponent("/repo/shot.png")}`;
const attachment = { src: "data:image/png;base64,QVRUQUNI", name: "attachment.png" };

beforeEach(() => {
    vi.mocked(fsapi.previewFile).mockResolvedValue({ mime: "image/png", size: 4_634_596, modified: 0 });
    vi.mocked(fsapi.downloadsDir).mockResolvedValue("/Users/me/Downloads");
    vi.mocked(fsapi.copyIntoDir).mockResolvedValue("/Users/me/Downloads/shot.png");
    vi.mocked(fsapi.saveBase64IntoDir).mockResolvedValue("/Users/me/Downloads/attachment.png");
});

afterEach(() => {
    act(() => hideImage());
    cleanup();
    vi.clearAllMocks();
    useToasts.setState({ toasts: [] });
});

describe("ImageViewer", () => {
    it("shows nothing until a picture is opened", () => {
        render(<ImageViewer />);
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

        act(() => showImage(shot));
        expect(screen.getByRole("dialog", { name: "shot.png" })).toBeInTheDocument();
        expect(screen.getByRole("img", { name: "shot.png" })).toHaveAttribute("src", shot.src);
    });

    /* The transcript's thumbnail of a big picture is a shrunk copy, so the
       viewer reads the file to show what was actually attached. */
    it("reads the file again to replace the thumbnail with the whole picture", async () => {
        render(<ImageViewer />);
        act(() => showImage(shot));

        await waitFor(() => expect(screen.getByRole("img", { name: "shot.png" })).toHaveAttribute("src", fullShot));
        expect(fsapi.previewFile).toHaveBeenCalledWith("/repo/shot.png");
    });

    it("keeps showing a picture that never came from a file", async () => {
        render(<ImageViewer />);
        act(() => showImage(attachment));

        await waitFor(() => expect(screen.getByRole("img", { name: "attachment.png" })).toHaveAttribute("src", attachment.src));
        expect(fsapi.previewFile).not.toHaveBeenCalled();
    });

    it("closes from the button, from Escape and from the scrim behind it", async () => {
        const user = userEvent.setup();
        render(<ImageViewer />);

        act(() => showImage(shot));
        await user.click(screen.getByRole("button", { name: "Close" }));
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

        act(() => showImage(shot));
        await user.keyboard("{Escape}");
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

        act(() => showImage(shot));
        await user.click(document.querySelector(".img-scrim")!);
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("stays open when the picture itself is clicked", async () => {
        const user = userEvent.setup();
        render(<ImageViewer />);
        act(() => showImage(shot));

        await user.click(screen.getByRole("img", { name: "shot.png" }));
        expect(screen.getByRole("dialog")).toBeInTheDocument();
    });

    /* A picture that is already a file is copied into the folder rather than
       read back through the window and written out again. */
    it("saves a picture that came from a file by copying it into Downloads", async () => {
        const user = userEvent.setup();
        render(<ImageViewer />);
        act(() => showImage(shot));

        await user.click(screen.getByRole("button", { name: "Save to Downloads" }));

        await waitFor(() => expect(fsapi.copyIntoDir).toHaveBeenCalledWith("/repo/shot.png", "/Users/me/Downloads"));
        expect(fsapi.saveBase64IntoDir).not.toHaveBeenCalled();
        await waitFor(() => expect(useToasts.getState().toasts[0]?.text).toBe("Saved shot.png to Downloads"));
    });

    it("saves a picture that only ever existed in the transcript out of its own bytes", async () => {
        const user = userEvent.setup();
        render(<ImageViewer />);
        act(() => showImage(attachment));

        await user.click(screen.getByRole("button", { name: "Save to Downloads" }));

        await waitFor(() => expect(fsapi.saveBase64IntoDir).toHaveBeenCalledWith("/Users/me/Downloads", "attachment.png", "QVRUQUNI"));
        expect(fsapi.copyIntoDir).not.toHaveBeenCalled();
    });

    it("says so when the folder will not take it", async () => {
        const user = userEvent.setup();
        vi.mocked(fsapi.copyIntoDir).mockRejectedValue(new Error("no room"));
        render(<ImageViewer />);
        act(() => showImage(shot));

        await user.click(screen.getByRole("button", { name: "Save to Downloads" }));

        await waitFor(() => expect(useToasts.getState().toasts[0]?.kind).toBe("error"));
        expect(useToasts.getState().toasts[0]?.text).toContain("no room");
        // The picture stays up, so the save can be tried again.
        expect(screen.getByRole("dialog")).toBeInTheDocument();
    });
});
