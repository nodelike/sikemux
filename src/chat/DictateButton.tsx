import type { RefObject } from "react";
import { IconMic } from "../components/Icons";
import { IS_MACOS } from "../lib/platform";
import { toggleDictation, useVoice } from "../voice/dictation";

export function DictateButton({ into }: { into: RefObject<HTMLElement | null> }) {
    const phase = useVoice((s) => s.phase);
    const mine = useVoice((s) => s.target !== null && s.target === into.current);
    if (!IS_MACOS || phase === "unsupported") return null;
    const listening = mine && phase === "listening";
    const writing = mine && phase === "transcribing";
    const label = listening ? "Stop and type what you said" : writing ? "Writing what you said" : "Dictate";
    return (
        <button
            type="button"
            className={`chat-composer-icon chat-dictate${listening ? " listening" : ""}`}
            aria-label={label}
            aria-pressed={listening}
            title={listening ? label : "Dictate — or hold right ⌥ anywhere"}
            disabled={writing}
            onClick={() => into.current && toggleDictation(into.current)}>
            <IconMic size={16} />
        </button>
    );
}
