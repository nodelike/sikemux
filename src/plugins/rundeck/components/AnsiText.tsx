import { memo } from "react";
import { parseAnsi } from "./ansi";

export const AnsiText = memo(function AnsiText({ text, className }: { text: string; className?: string }) {
    const segments = parseAnsi(text);
    return (
        <span className={className}>
            {segments.map((segment, index) => {
                const classes = [
                    segment.fg !== null ? `rnd-ansi-fg-${segment.fg}` : "",
                    segment.bg !== null ? `rnd-ansi-bg-${segment.bg}` : "",
                    segment.bold ? "rnd-ansi-bold" : "",
                ]
                    .filter(Boolean)
                    .join(" ");
                return classes ? (
                    <span key={index} className={classes}>
                        {segment.text}
                    </span>
                ) : (
                    segment.text
                );
            })}
        </span>
    );
});
