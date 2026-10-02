import { describe, expect, it } from "vitest";
import { contextAsText, contextChip, splitSentContext } from "./promptContext";

const issue = { uri: "https://github.com/o/r/issues/12", title: "Login crashes", text: "Issue #12: Login crashes\n\nIt crashes." };

describe("contextChip", () => {
    it("reads the kind and number of an issue or pull request from its address", () => {
        expect(contextChip(issue)).toEqual({ kind: "issue", number: "12", title: "Login crashes" });
        expect(contextChip({ uri: "https://github.com/o/r/pull/7", title: "Pull request #7: Faster boot" })).toEqual({
            kind: "pull",
            number: "7",
            title: "Faster boot",
        });
    });

    it("names anything else by its title", () => {
        expect(contextChip({ uri: "https://example.com/doc", title: "Notes" })).toEqual({ kind: null, number: null, title: "Notes" });
    });
});

describe("contextAsText", () => {
    it("writes a titled, fenced section", () => {
        expect(contextAsText(issue)).toBe("### Login crashes\nhttps://github.com/o/r/issues/12\n\n```\nIssue #12: Login crashes\n\nIt crashes.\n```");
    });

    it("fences past any backticks in the text", () => {
        expect(contextAsText({ ...issue, text: "```js\nx\n```" })).toContain("\n````\n```js");
    });
});

describe("splitSentContext", () => {
    it("reads back sections written into the text", () => {
        expect(splitSentContext(`fix this\n\n${contextAsText(issue)}`)).toEqual({
            text: "fix this",
            context: [{ uri: issue.uri, title: "Login crashes" }],
        });
    });

    it("reads back what Claude's adapter wrote for an embedded item", () => {
        const sent = `fix this${issue.uri}\n<context ref="${issue.uri}">\n${issue.text}\n</context>`;
        expect(splitSentContext(sent)).toEqual({ text: "fix this", context: [{ uri: issue.uri, title: "Issue #12: Login crashes" }] });
    });

    it("leaves ordinary text alone", () => {
        expect(splitSentContext("### Heading\nnot a section")).toEqual({ text: "### Heading\nnot a section", context: [] });
    });
});
