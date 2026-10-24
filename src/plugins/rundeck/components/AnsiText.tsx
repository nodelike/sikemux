import { memo, type CSSProperties } from "react";
import type { RundeckStyledText, RundeckTermColor } from "../types.generated";

function colourClass(side: "fg" | "bg", colour: RundeckTermColor | null): string {
    return colour?.kind === "palette" && colour.index < 16 ? `rnd-ansi-${side}-${colour.index}` : "";
}

function exact(colour: RundeckTermColor | null): string | undefined {
    return colour?.kind === "rgb" ? `rgb(${colour.r}, ${colour.g}, ${colour.b})` : undefined;
}

/** A line of output in the styles the plugin read from its terminal codes. */
export const AnsiText = memo(function AnsiText({ text, runs, className }: { text: string; runs?: RundeckStyledText[]; className?: string }) {
    if (!runs?.length) return <span className={className}>{text}</span>;
    return (
        <span className={className}>
            {runs.map((run, index) => {
                const classes = [colourClass("fg", run.fg), colourClass("bg", run.bg), run.bold ? "rnd-ansi-bold" : ""].filter(Boolean).join(" ");
                const style: CSSProperties = {
                    color: exact(run.fg),
                    background: exact(run.bg),
                    opacity: run.dim ? 0.7 : undefined,
                    fontStyle: run.italic ? "italic" : undefined,
                    textDecoration: run.underline ? "underline" : undefined,
                };
                return (
                    <span key={index} className={classes || undefined} style={style}>
                        {run.text}
                    </span>
                );
            })}
        </span>
    );
});
