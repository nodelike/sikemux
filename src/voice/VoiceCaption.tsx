import { useLayoutEffect, useRef, type CSSProperties } from "react";
import { useVoice } from "./dictation";

const INSET = 12;

/** Where the caption goes: inside the bottom of the field. A chat's composer shows the words in its own box instead. */
export function captionStyle(target: HTMLElement | null, viewportHeight: number): CSSProperties {
    if (!target?.isConnected) return { left: "50%", bottom: 64 };
    const rect = target.getBoundingClientRect();
    return { left: rect.left + rect.width / 2, bottom: viewportHeight - rect.bottom + INSET, maxWidth: Math.max(rect.width - INSET * 2, 160) };
}

/** Keeps the newest words in view as they arrive, unless the person has scrolled up to read earlier ones. */
export function useFollowNewest(text: string) {
    const lines = useRef<HTMLDivElement>(null);
    const following = useRef(true);
    useLayoutEffect(() => {
        if (!text) following.current = true;
        const element = lines.current;
        if (element && following.current) element.scrollTop = element.scrollHeight;
    }, [text]);
    const onScroll = () => {
        const element = lines.current;
        if (element) following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 4;
    };
    return { lines, onScroll };
}

export function VoiceCaption() {
    const partial = useVoice((s) => s.partial);
    const target = useVoice((s) => s.target);
    const phase = useVoice((s) => s.phase);
    const { lines, onScroll } = useFollowNewest(partial);
    if (!partial || (phase !== "listening" && phase !== "transcribing")) return null;
    if (target?.querySelector(".chat-composer")) return null;

    return (
        <div
            className={`voice-caption${phase === "transcribing" ? " settling" : ""}`}
            style={captionStyle(target, window.innerHeight)}
            role="status"
            aria-live="polite">
            <div className="voice-caption-lines" ref={lines} onScroll={onScroll}>
                <p>{partial}</p>
            </div>
        </div>
    );
}
