import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChatMessageRow } from "./ChatMessageRow";
import type { ChatMessage } from "./types";

vi.mock("../api/fs", () => {
    const fsapi = {
        pathKinds: vi.fn(async (paths: string[]) => paths.map(() => null)),
        previewFile: vi.fn(async () => Promise.reject(new Error("none"))),
    };
    return { fsapi, filesOn: () => fsapi };
});

const issue = { uri: "https://github.com/o/r/issues/12", title: "#12 Login crashes" };

function show(message: ChatMessage) {
    render(<ChatMessageRow message={message} live={false} copyable="" rate={null} at={null} took={null} />);
}

afterEach(cleanup);

describe("ChatMessageRow context", () => {
    it("shows what a message was sent with as a chip beside its own words", async () => {
        show({ id: "m", role: "user", parts: [{ id: "t", kind: "text", text: "fix it" }], context: [{ uri: issue.uri, title: issue.title }] });
        expect(screen.getByTitle(issue.uri)).toHaveTextContent("#12");
        expect(screen.getByTitle(issue.uri)).toHaveTextContent("Login crashes");
        expect(await screen.findByText("fix it")).toBeInTheDocument();
    });
});
