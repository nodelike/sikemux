import type { CSSProperties } from "react";
import { useVoice } from "./dictation";

const INSET = 12;

export function VoiceCaption() {
    const partial = useVoice((s) => s.partial);
    const target = useVoice((s) => s.target);
    const phase = useVoice((s) => s.phase);
    if (!partial || (phase !== "listening" && phase !== "transcribing")) return null;

    const rect = target?.isConnected ? target.getBoundingClientRect() : null;
    const style: CSSProperties = rect
        ? { left: rect.left + rect.width / 2, bottom: window.innerHeight - rect.bottom + INSET, maxWidth: Math.max(rect.width - INSET * 2, 160) }
        : { left: "50%", bottom: 64 };

    return (
        <div className={`voice-caption${phase === "transcribing" ? " settling" : ""}`} style={style} role="status" aria-live="polite">
            <div className="voice-caption-lines">
                <p>{partial}</p>
            </div>
        </div>
    );
}
