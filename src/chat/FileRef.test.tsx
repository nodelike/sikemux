import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    pathKinds: vi.fn(),
    revealInFinder: vi.fn(async () => {}),
    requestOpenFile: vi.fn(),
    openFileOnDesk: vi.fn(),
    copyText: vi.fn(async () => {}),
}));

vi.mock("../api/fs", () => ({
    fsapi: { pathKinds: mocks.pathKinds, revealInFinder: mocks.revealInFinder },
}));
vi.mock("../state/commands", () => ({ requestOpenFile: mocks.requestOpenFile, openFileOnDesk: mocks.openFileOnDesk }));
vi.mock("../lib/clipboard", () => ({ copyText: mocks.copyText }));

const { ChatFileRef, PathRootsProvider, useFileRef } = await import("./FileRef");
const { pathComponents } = await import("./markdownPaths");
const { Markdown } = await import("../markdown/Markdown");
const { forgetPathState } = await import("./pathExistence");

const CWD = "/work/demo";
const EXISTING = new Set(["/work/demo/src/a.ts", "/work/demo/README.md", "/work/demo/src"]);
const DIRS = new Set(["/work/demo/src"]);

function Link({ href, guess, children }: { href: string; guess?: "text" | "code"; children?: React.ReactNode }) {
    const file = useFileRef(href);
    if (file)
        return (
            <ChatFileRef
                refers={file.ref}
                state={file.state}
                label={children}
                className={guess === "code" ? "chat-file-ref code" : "chat-file-ref link"}
            />
        );
    if (guess === "code") return <code>{children}</code>;
    if (guess === "text") return <>{children}</>;
    return <a href={href}>{children}</a>;
}

const components = { link: Link, ...pathComponents(Link) };
const OPTIONS = { gfm: true, htmlAsText: false, fileLinks: true };

/* A transcript the app draws the way the pane does: the same path links, the
   same link component, the same project root. */
function Body({ text }: { text: string }) {
    return (
        <PathRootsProvider cwd={CWD}>
            <div className="chat-markdown">
                <Markdown text={text} options={OPTIONS} components={components} />
            </div>
        </PathRootsProvider>
    );
}

beforeEach(() => {
    vi.clearAllMocks();
    forgetPathState();
    mocks.pathKinds.mockImplementation(async (paths: string[]) =>
        paths.map((path) => (EXISTING.has(path) ? (DIRS.has(path) ? "dir" : "file") : null)),
    );
});

afterEach(cleanup);

describe("files a message names", () => {
    it("turns a path written plainly into something to open", async () => {
        render(<Body text="I edited src/a.ts to fix it." />);
        const link = await screen.findByRole("button", { name: /src\/a\.ts/ });
        expect(link).toHaveAttribute("title", "/work/demo/src/a.ts");
        fireEvent.click(link);
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", undefined, undefined);
    });

    it("opens at the line the message named", async () => {
        render(<Body text="see src/a.ts:42 for the cause" />);
        fireEvent.click(await screen.findByRole("button", { name: /src\/a\.ts:42/ }));
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", 41, undefined);
    });

    it("leaves the words around it to the sentence", async () => {
        render(<Body text="I edited src/a.ts." />);
        const chip = await screen.findByRole("button", { name: /src\/a\.ts/ });
        expect(chip.previousSibling?.textContent).toBe("I edited ");
        expect(chip.nextSibling?.textContent).toBe(".");
    });

    it("leaves a name with no file behind it as the words the agent wrote", async () => {
        const { container } = render(<Body text="I would put it in src/gone.ts next." />);
        await waitFor(() => expect(mocks.pathKinds).toHaveBeenCalled());
        await waitFor(() => expect(container.querySelector("button")).toBeNull());
        expect(container.textContent).toBe("I would put it in src/gone.ts next.");
    });

    it("gives a path in backticks the same treatment", async () => {
        render(<Body text="Look at `src/a.ts` again." />);
        expect(await screen.findByRole("button", { name: /src\/a\.ts/ })).toBeInTheDocument();
    });

    it("puts a name that is not a file back between its backticks", async () => {
        const { container } = render(<Body text="Run `npm/nope` first." />);
        await waitFor(() => expect(mocks.pathKinds).toHaveBeenCalled());
        await waitFor(() => expect(container.querySelector("code")).not.toBeNull());
        expect(container.querySelector("code")?.textContent).toBe("npm/nope");
    });

    it("keeps a markdown link to a file, under the words it was given", async () => {
        render(<Body text="[the file I changed](src/a.ts)" />);
        const link = await screen.findByRole("button", { name: /the file I changed/ });
        fireEvent.click(link);
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", undefined, undefined);
    });

    it("leaves a web address alone", async () => {
        render(<Body text="see https://example.com/a for more" />);
        const link = await screen.findByRole("link");
        expect(link).toHaveAttribute("href", "https://example.com/a");
    });

    it("reveals a folder rather than opening it as text", async () => {
        render(<Body text="everything lives under src/ now" />);
        fireEvent.click(await screen.findByRole("button", { name: /src/ }));
        expect(mocks.requestOpenFile).not.toHaveBeenCalled();
        expect(mocks.revealInFinder).toHaveBeenCalledWith("/work/demo/src");
    });

    it("asks about every file on screen in one go", async () => {
        render(<Body text="src/a.ts and README.md and src/gone.ts" />);
        await waitFor(() => expect(mocks.pathKinds).toHaveBeenCalled());
        expect(mocks.pathKinds).toHaveBeenCalledTimes(1);
        expect(mocks.pathKinds.mock.calls[0][0]).toEqual(["/work/demo/src/a.ts", "/work/demo/README.md", "/work/demo/src/gone.ts"]);
    });
});

describe("the menu a file opens on right-click", () => {
    async function openMenu() {
        render(<Body text="I edited src/a.ts." />);
        fireEvent.contextMenu(await screen.findByRole("button", { name: /src\/a\.ts/ }));
        return screen.findByRole("menu");
    }

    it("offers to reveal the file where the system shows files", async () => {
        const menu = await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: /Reveal in/ }));
        expect(mocks.revealInFinder).toHaveBeenCalledWith("/work/demo/src/a.ts");
        expect(menu).toBeTruthy();
    });

    it("offers to open the file", async () => {
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Open" }));
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", undefined, undefined);
    });

    it("copies the path as the project sees it and as the system does", async () => {
        await openMenu();
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy Relative Path" }));
        expect(mocks.copyText).toHaveBeenCalledWith("src/a.ts");
        fireEvent.contextMenu(screen.getByRole("button", { name: /src\/a\.ts/ }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy Path" }));
        expect(mocks.copyText).toHaveBeenCalledWith("/work/demo/src/a.ts");
    });
});

describe("a file named in an agent's own chat", () => {
    function Chip({ raw }: { raw: string | null }) {
        const file = useFileRef(raw);
        return file ? <ChatFileRef refers={file.ref} state={file.state} label={raw} /> : <span>plain</span>;
    }

    function OnDesk({ raw }: { raw: string | null }) {
        return (
            <PathRootsProvider cwd={CWD} agentId="agent-1">
                <Chip raw={raw} />
            </PathRootsProvider>
        );
    }

    it("puts a click on the agent's desk at the named line and column", async () => {
        render(<OnDesk raw="src/a.ts:12:4" />);
        const chip = await screen.findByRole("button", { name: /src\/a\.ts/ });
        expect(chip).toHaveAttribute("title", "/work/demo/src/a.ts:12");

        fireEvent.click(chip, { detail: 1 });
        expect(mocks.openFileOnDesk).toHaveBeenCalledWith("agent-1", "/work/demo/src/a.ts", 11, 3);
        expect(mocks.requestOpenFile).not.toHaveBeenCalled();
    });

    it("opens a double-clicked file in the editor without a second trip to the desk", async () => {
        render(<OnDesk raw="src/a.ts" />);
        const chip = await screen.findByRole("button", { name: /src\/a\.ts/ });
        fireEvent.click(chip, { detail: 1 });
        fireEvent.click(chip, { detail: 2 });
        fireEvent.doubleClick(chip);

        expect(mocks.openFileOnDesk).toHaveBeenCalledTimes(1);
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", undefined, undefined);
    });

    it("only reveals a double-clicked folder", async () => {
        render(<OnDesk raw="src/" />);
        const chip = await screen.findByRole("button", { name: /src/ });
        fireEvent.doubleClick(chip);

        expect(mocks.requestOpenFile).not.toHaveBeenCalled();
        expect(mocks.openFileOnDesk).not.toHaveBeenCalled();
        expect(chip).toHaveAttribute("data-kind", "dir");
    });

    it("offers the desk and the editor separately in the menu", async () => {
        render(<OnDesk raw="src/a.ts" />);
        fireEvent.contextMenu(await screen.findByRole("button", { name: /src\/a\.ts/ }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Open on Desk" }));
        expect(mocks.openFileOnDesk).toHaveBeenCalledWith("agent-1", "/work/demo/src/a.ts", undefined, undefined);

        fireEvent.contextMenu(screen.getByRole("button", { name: /src\/a\.ts/ }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Open in Editor" }));
        expect(mocks.requestOpenFile).toHaveBeenCalledWith("/work/demo/src/a.ts", undefined, undefined);

        fireEvent.contextMenu(screen.getByRole("button", { name: /src\/a\.ts/ }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Open Containing Folder" }));
        expect(mocks.revealInFinder).toHaveBeenCalledWith("/work/demo/src");
    });

    it("offers a folder only the ways a folder can be used", async () => {
        render(<OnDesk raw="src/" />);
        fireEvent.contextMenu(await screen.findByRole("button", { name: /src/ }));

        expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
            expect.stringMatching(/^Reveal in /),
            "Copy Path",
            "Copy Relative Path",
            "Copy Name",
        ]);
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy Name" }));
        expect(mocks.copyText).toHaveBeenCalledWith("src");
    });

    it("copies a file outside the project by its name when asked for its relative path", async () => {
        EXISTING.add("/etc/hosts");
        try {
            render(<OnDesk raw="/etc/hosts" />);
            fireEvent.contextMenu(await screen.findByRole("button", { name: /\/etc\/hosts/ }));
            fireEvent.click(screen.getByRole("menuitem", { name: "Copy Relative Path" }));
            expect(mocks.copyText).toHaveBeenCalledWith("hosts");
        } finally {
            EXISTING.delete("/etc/hosts");
        }
    });

    it("draws nothing to open for no reference at all", () => {
        render(<OnDesk raw={null} />);
        expect(screen.getByText("plain")).toBeInTheDocument();
        expect(mocks.pathKinds).not.toHaveBeenCalled();
    });
});
