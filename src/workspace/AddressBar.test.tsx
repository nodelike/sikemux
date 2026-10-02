import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserApi, type AddressSuggestions } from "../api/browser";
import { AddressBar } from "./AddressBar";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { suggest: vi.fn() } };
});

const youtube: AddressSuggestions = {
    completion: { url: "https://www.youtube.com/", title: "YouTube", address: "youtube.com/", icon: null },
    pages: [
        { url: "https://www.youtube.com/watch?v=abc", title: "KREAM - YouTube", address: "youtube.com/watch?v=abc", icon: null },
        {
            url: "https://studio.youtube.com/analytics",
            title: "Video analytics - YouTube Studio",
            address: "studio.youtube.com/analytics",
            icon: null,
        },
    ],
    searches: true,
    searchUrl: "https://www.google.com/search?q=you",
};

const onGo = vi.fn();
function renderBar(pageAddress = "https://example.com/") {
    render(<AddressBar tabId="tab-one" pageAddress={pageAddress} onGo={onGo} />);
    return screen.getByRole("textbox", { name: "Address and search" }) as HTMLInputElement;
}

async function type(input: HTMLInputElement, value: string, inputType = "insertText") {
    await act(async () => {
        fireEvent.input(input, { target: { value }, inputType });
    });
}

beforeEach(() => {
    vi.mocked(browserApi.suggest).mockResolvedValue(youtube);
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("AddressBar", () => {
    it("finishes a remembered site in place and selects the part it added", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        expect(browserApi.suggest).toHaveBeenCalledWith("you");
        expect(input).toHaveValue("youtube.com/");
        expect([input.selectionStart, input.selectionEnd]).toEqual([3, 12]);

        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("https://www.youtube.com/");
        expect(input).toHaveValue("https://example.com/");
    });

    it("lists the matching pages under the field, finished site first and a search last", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        const options = screen.getAllByRole("option");
        expect(options.map((option) => option.textContent)).toEqual([
            "YouTube — youtube.com/",
            "KREAM - YouTube — youtube.com/watch?v=abc",
            "Video analytics - YouTube Studio — studio.youtube.com/analytics",
            "you — Google Search",
        ]);
        expect(options[0]).toHaveAttribute("aria-selected", "true");
    });

    it("walks the list with the arrow keys and opens the one chosen", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        fireEvent.keyDown(input, { key: "ArrowDown" });
        expect(input).toHaveValue("youtube.com/watch?v=abc");
        expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");

        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("https://www.youtube.com/watch?v=abc");
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("closes the list when a click lands anywhere else, even on something that takes no focus", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");
        expect(screen.getByRole("listbox")).toBeInTheDocument();

        fireEvent.pointerDown(document.body);

        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
        expect(input).not.toHaveFocus();
        expect(input).toHaveValue("https://example.com/");
    });

    it("closes the list when a click on the page takes the window's focus", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        act(() => {
            window.dispatchEvent(new Event("blur"));
        });

        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("keeps the list open while the pointer goes down on a suggestion", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        fireEvent.pointerDown(screen.getAllByRole("option")[1]);

        expect(screen.getByRole("listbox")).toBeInTheDocument();
    });

    it("opens a page that is clicked without the field losing focus first", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");

        const search = screen.getByRole("option", { name: /Google Search/ });
        expect(fireEvent.mouseDown(search)).toBe(false);
        fireEvent.click(search);
        expect(onGo).toHaveBeenCalledWith("https://www.google.com/search?q=you");
    });

    it("does not finish the address again after the finished part is deleted", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");
        await type(input, "you", "deleteContentBackward");

        expect(input).toHaveValue("you");
        expect(screen.getAllByRole("option")[0]).toHaveTextContent("you — Google Search");
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("https://www.google.com/search?q=you");
    });

    it("sends what was typed when nothing is remembered", async () => {
        vi.mocked(browserApi.suggest).mockResolvedValue({ completion: null, pages: [], searches: false, searchUrl: "" });
        const input = renderBar();
        input.focus();
        await type(input, "openai.com");

        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("openai.com");
    });

    it("ignores an answer that arrives after the typing has moved on", async () => {
        let answerFirst: (value: AddressSuggestions) => void = () => {};
        const input = renderBar();
        input.focus();
        vi.mocked(browserApi.suggest)
            .mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)))
            .mockResolvedValueOnce({ ...youtube, completion: null, pages: [] });
        await type(input, "y");
        await type(input, "yo");
        await act(async () => answerFirst(youtube));

        expect(input).toHaveValue("yo");
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    const topSites: AddressSuggestions = {
        completion: null,
        pages: [
            { url: "https://example.com/", title: "Example", address: "example.com/", icon: null },
            { url: "https://www.youtube.com/", title: "YouTube", address: "youtube.com/", icon: null },
            { url: "https://github.com/", title: "GitHub", address: "github.com/", icon: null },
        ],
        searches: false,
        searchUrl: "",
    };

    it("offers the most visited sites on focus, with none picked", async () => {
        vi.mocked(browserApi.suggest).mockResolvedValue(topSites);
        const input = renderBar("https://www.example.com/?zx=1790764778073");
        await act(async () => input.focus());

        expect(browserApi.suggest).toHaveBeenCalledWith("");
        const options = screen.getAllByRole("option");
        expect(options.map((option) => option.textContent)).toEqual(["Example — example.com/", "YouTube — youtube.com/", "GitHub — github.com/"]);
        expect(options.every((option) => option.getAttribute("aria-selected") === "false")).toBe(true);
        expect(input).toHaveValue("https://www.example.com/?zx=1790764778073");

        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("https://www.example.com/?zx=1790764778073");
    });

    it("leaves the page already open out of the top sites", async () => {
        vi.mocked(browserApi.suggest).mockResolvedValue(topSites);
        const input = renderBar("https://example.com/");
        await act(async () => input.focus());

        expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["YouTube — youtube.com/", "GitHub — github.com/"]);
    });

    it("picks a top site with the arrow keys and backs out to the open address above the first", async () => {
        vi.mocked(browserApi.suggest).mockResolvedValue(topSites);
        const input = renderBar();
        await act(async () => input.focus());

        fireEvent.keyDown(input, { key: "ArrowDown" });
        expect(input).toHaveValue("youtube.com/");
        fireEvent.keyDown(input, { key: "ArrowUp" });
        expect(input).toHaveValue("https://example.com/");
        expect(screen.getAllByRole("option").every((option) => option.getAttribute("aria-selected") === "false")).toBe(true);

        fireEvent.keyDown(input, { key: "ArrowDown" });
        fireEvent.keyDown(input, { key: "ArrowDown" });
        fireEvent.keyDown(input, { key: "Enter" });
        expect(onGo).toHaveBeenCalledWith("https://github.com/");
    });

    it("offers the top sites again once the field is emptied", async () => {
        const input = renderBar();
        input.focus();
        await type(input, "you");
        vi.mocked(browserApi.suggest).mockResolvedValue(topSites);
        await type(input, "", "deleteContentBackward");

        expect(browserApi.suggest).toHaveBeenLastCalledWith("");
        expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["YouTube — youtube.com/", "GitHub — github.com/"]);
    });
});
