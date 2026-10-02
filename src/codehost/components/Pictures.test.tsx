import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ image: vi.fn<(url: string) => Promise<string>>() }));
vi.mock("../../api/markdown", () => ({
    markdownApi: {
        parse: () =>
            Promise.resolve([
                {
                    t: "p",
                    c: [
                        { t: "img", src: "https://github.com/a.png", alt: "chart" },
                        { t: "img", src: "https://github.com/b.png", alt: "gone" },
                    ],
                },
            ]),
    },
}));

import { Face } from "./CommentThread";
import { Avatar, Prose } from "./Pictures";
import { InHost, registerTestHost } from "../testHost";

const host = registerTestHost(api);
const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={host}>{children}</InHost>;

afterEach(cleanup);

describe("pictures from GitHub", () => {
    it("holds an avatar's place until its picture arrives", async () => {
        let deliver: (data: string) => void = () => {};
        api.image.mockReturnValueOnce(new Promise((resolve) => (deliver = resolve)));
        const { container } = render(<Avatar url="https://avatars.githubusercontent.com/u/1" />, { wrapper });
        expect(container.innerHTML).toBe('<span class="gha-avatar" aria-hidden="true"></span>');
        await act(async () => deliver("data:image/png;base64,AA=="));
        expect(container.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,AA==");
    });

    it("finds a picture from a name alone, and falls back to the first letter when it cannot load", async () => {
        const named = { ...host, avatarForLogin: (login: string) => `https://avatars.example/${login}` };
        api.image.mockRejectedValueOnce(new Error("offline"));
        const { container } = render(
            <InHost host={named}>
                <Face login="nodelike" url={null} />
            </InHost>,
        );
        expect(api.image).toHaveBeenLastCalledWith("https://avatars.example/nodelike");
        await waitFor(() => expect(container.textContent).toBe("N"));
    });

    it("shows a picture's description as a link until it loads, and for good if it cannot", async () => {
        api.image.mockImplementation((url) =>
            url.endsWith("a.png") ? Promise.resolve("data:image/png;base64,AA==") : Promise.reject(new Error("no")),
        );
        const { container } = render(<Prose>{"![chart](https://github.com/a.png) ![gone](https://github.com/b.png)"}</Prose>, { wrapper });
        await waitFor(() => expect(container.querySelector("img")?.getAttribute("alt")).toBe("chart"));
        const link = container.querySelector("a");
        expect(link?.textContent).toBe("gone");
        expect(link?.getAttribute("href")).toBe("https://github.com/b.png");
    });
});
