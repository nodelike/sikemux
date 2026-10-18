import { describe, expect, it } from "vitest";
import { promptDraft } from "./promptDraft";

describe("what is typed in an agent's TUI", () => {
    it("reads Claude's input box, its continuation lines and not the transcript above", () => {
        const screen = [
            "❯ fix the login flake",
            "⏺ Done. The test now waits for the page.",
            "",
            "────────────────────────────────────────",
            "❯ now check the logout path too",
            "  and the session timeout",
            "────────────────────────────────────────",
            "  ⏵⏵ bypass permissions on (shift+tab to cycle)",
        ];
        expect(promptDraft(screen)).toBe("now check the logout path too\nand the session timeout");
    });

    it("reads Claude's box as it looks on screen", () => {
        const screen = [
            "                                                     ctrl+g to edit in VS Code",
            "────────────────────────────────────────────────",
            "❯ write the migration",
            "  and its rollback",
            "────────────────────────────────────────────────",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ];
        expect(promptDraft(screen)).toBe("write the migration\nand its rollback");
    });

    it("reads an empty box, a menu and a screen with no box as nothing", () => {
        expect(promptDraft(["────────────", "❯", "────────────"])).toBe("");
        const trust = [
            "────────────",
            " Accessing workspace:",
            " Security guide",
            " ❯ No, exit",
            "   Yes, I trust this folder",
            " Enter to confirm · Esc to cancel",
        ];
        expect(promptDraft(trust)).toBe("");
        expect(promptDraft(["$ ls", "README.md  src"])).toBe("");
    });
});
