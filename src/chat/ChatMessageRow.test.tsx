import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChatMessageRow } from "./ChatMessageRow";
import { contextAsText } from "./promptContext";
import type { ChatMessage } from "./types";

vi.mock("../api/fs", () => ({
    fsapi: { pathKinds: vi.fn(async (paths: string[]) => paths.map(() => null)), previewFile: vi.fn(async () => Promise.reject(new Error("none"))) },
}));

const issue = { uri: "https://github.com/o/r/issues/12", title: "#12 Login crashes", text: "Issue #12: Login crashes\n\nA long body." };

function show(message: ChatMessage) {
    render(<ChatMessageRow message={message} live={false} copyable="" rate={null} at={null} took={null} />);
}

afterEach(cleanup);

describe("ChatMessageRow context", () => {
    it("shows a sent issue as a chip with its number and title", () => {
        show({ id: "m", role: "user", parts: [{ id: "t", kind: "text", text: "fix it" }], context: [issue] });
        const chip = screen.getByTitle(issue.uri);
        expect(chip).toHaveTextContent("#12");
        expect(chip).toHaveTextContent("Login crashes");
    });

    it("folds an issue written into a replayed message back into a chip", () => {
        show({ id: "m", role: "user", parts: [{ id: "t", kind: "text", text: `fix it\n\n${contextAsText(issue)}` }] });
        expect(screen.getByTitle(issue.uri)).toHaveTextContent("#12");
        expect(screen.queryByText(/A long body/)).not.toBeInTheDocument();
    });

    it("shows a replayed embedded resource as a chip", () => {
        show({
            id: "m",
            role: "user",
            parts: [{ id: "r", kind: "content", content: { type: "resource", resource: { uri: issue.uri, text: issue.text } } }],
        });
        expect(screen.getByTitle(issue.uri)).toHaveTextContent("Login crashes");
        expect(screen.queryByText(/A long body/)).not.toBeInTheDocument();
    });
});
