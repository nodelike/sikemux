import type { StoreState } from "../state/store";
import { selectTabRefs } from "../state/selectors";

const AGENT_NAMES = ["Claude", "Codex", "Hermes", "OpenCode", "Gemini", "Grok"];
const MAX_TERMS = 100;

function basename(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? "";
}

function withoutExtension(name: string): string {
    const dot = name.lastIndexOf(".");
    return dot > 0 ? name.slice(0, dot) : name;
}

/** Words the speech model should prefer: the user's own list, then names from the project on screen. */
export function voiceVocabulary(state: StoreState): string[] {
    const terms: string[] = [...state.voiceWords, "Sikemux", ...AGENT_NAMES];
    const session = state.sessions[state.activeSessionId];
    if (session) {
        terms.push(session.name, basename(session.cwd));
        for (const ref of selectTabRefs(state, session.id)) {
            if (!ref.doc) continue;
            const name = basename(ref.doc);
            terms.push(name, withoutExtension(name));
        }
    }
    const seen = new Set<string>();
    const unique: string[] = [];
    for (const raw of terms) {
        const term = raw.trim();
        const key = term.toLowerCase();
        if (term.length < 3 || seen.has(key)) continue;
        seen.add(key);
        unique.push(term);
        if (unique.length === MAX_TERMS) break;
    }
    return unique;
}

export function parseVoiceWords(text: string): string[] {
    return text
        .split(/[,\n]/)
        .map((word) => word.trim())
        .filter(Boolean);
}
