import { useLayoutEffect, useRef, type RefObject } from "react";

/** Draws the box's text with the words still being spoken dimmed at the caret, where they will be typed. */
export default function DictationPreview({
    draft,
    caret,
    heard,
    editor,
}: {
    draft: string;
    caret: number;
    heard: string;
    editor: RefObject<{ scrollTop: number } | null>;
}) {
    const preview = useRef<HTMLDivElement>(null);
    const spoken = useRef<HTMLSpanElement>(null);
    useLayoutEffect(() => {
        const box = preview.current;
        const words = spoken.current;
        if (!box || !words) return;
        const bottom = box.getBoundingClientRect().bottom - parseFloat(getComputedStyle(box).paddingBottom);
        box.scrollTop += words.getBoundingClientRect().bottom - bottom;
        if (editor.current) editor.current.scrollTop = box.scrollTop;
    }, [draft, caret, heard, editor]);
    const before = draft.slice(0, caret);
    return (
        <div className="chat-dictation-preview" ref={preview}>
            {before}
            <span ref={spoken} aria-live="polite">
                {before && !/\s$/.test(before) ? " " : ""}
                {heard}
            </span>
            {draft.slice(caret)}
        </div>
    );
}
