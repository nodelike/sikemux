import { describe, expect, it } from "vitest";
import { getState } from "../state/store";
import { parseVoiceWords, voiceVocabulary } from "./vocabulary";

function stateWith(words: string[], name: string, cwd: string) {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    return {
        ...state,
        voiceWords: words,
        sessions: { ...state.sessions, [session.id]: { ...session, name, cwd } },
    };
}

describe("voiceVocabulary", () => {
    it("puts the user's words first, then the project and agent names", () => {
        const terms = voiceVocabulary(stateWith(["pnpm", "Tauri"], "sikemux", "/Users/me/proj/sikemux"));
        expect(terms.slice(0, 3)).toEqual(["pnpm", "Tauri", "Sikemux"]);
        expect(terms).toContain("Codex");
    });

    it("drops duplicates regardless of case, and words too short to spot", () => {
        const terms = voiceVocabulary(stateWith(["sikemux", "go", "Codex"], "Sikemux", "/tmp/SIKEMUX"));
        expect(terms.filter((term) => term.toLowerCase() === "sikemux")).toEqual(["sikemux"]);
        expect(terms.filter((term) => term === "Codex")).toHaveLength(1);
        expect(terms).not.toContain("go");
    });
});

describe("parseVoiceWords", () => {
    it("splits on commas and new lines and trims each word", () => {
        expect(parseVoiceWords(" pnpm, Tauri ,\nworktree,, ")).toEqual(["pnpm", "Tauri", "worktree"]);
    });
});
