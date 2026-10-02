import { cleanup, render, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthorAvatar, AuthorPicturesProvider, authorColor, initials, type AuthorPictures } from "./AuthorAvatar";

afterEach(cleanup);

describe("initials", () => {
    it("takes the first and last name's letters, or the start of a single name", () => {
        expect(initials("Ada Byron Lovelace")).toBe("AL");
        expect(initials("ada")).toBe("AD");
        expect(initials("   ")).toBe("?");
    });
});

describe("AuthorAvatar", () => {
    const withPictures = (pictures: AuthorPictures, node: ReactNode) => <AuthorPicturesProvider value={pictures}>{node}</AuthorPicturesProvider>;

    it("shows initials on the author's own colour without a code host", () => {
        const { container } = render(<AuthorAvatar name="Ada Lovelace" email="ada@example.test" />);
        const avatar = container.querySelector(".gg-avatar")!;
        expect(avatar).toHaveTextContent("AL");
        expect(avatar).toHaveStyle({ background: authorColor("ada@example.test") });
    });

    it("swaps in the host's picture once it loads, and reuses it without loading again", async () => {
        const pictures = { pictureFor: vi.fn(() => "https://host.test/ada.png"), load: vi.fn(async () => "data:image/png;base64,ADA") };
        const first = render(withPictures(pictures, <AuthorAvatar name="Ada" email="ada@example.test" />));
        expect(first.container.querySelector(".gg-avatar")).toHaveTextContent("AD");
        await waitFor(() => expect(first.container.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,ADA"));
        first.unmount();

        const second = render(withPictures(pictures, <AuthorAvatar name="Ada" email="ada@example.test" />));
        expect(second.container.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,ADA");
        expect(pictures.load).toHaveBeenCalledOnce();
    });

    it("keeps the initials when the host knows no account or the picture fails to load", async () => {
        const failing = { pictureFor: () => "https://host.test/broken.png", load: vi.fn(async () => Promise.reject(new Error("404"))) };
        const broken = render(withPictures(failing, <AuthorAvatar name="Bob" email="bob@example.test" />));
        await waitFor(() => expect(failing.load).toHaveBeenCalled());
        expect(broken.container.querySelector("img")).toBeNull();
        broken.unmount();

        const unknown = { pictureFor: vi.fn(() => null), load: vi.fn() };
        const none = render(withPictures(unknown, <AuthorAvatar name="Cy" email="" />));
        expect(unknown.pictureFor).not.toHaveBeenCalled();
        expect(unknown.load).not.toHaveBeenCalled();
        expect(none.container.querySelector(".gg-avatar")).toHaveStyle({ background: authorColor("Cy") });
    });
});
