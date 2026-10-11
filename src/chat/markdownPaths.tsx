import type { ComponentType, ReactNode } from "react";
import type { MarkdownComponents } from "../markdown/Markdown";
import { namesWholePath, scanPathCandidates } from "./filePath";

/** A link made from a name the message only mentioned: written plainly, or between backticks. */
export type PathGuess = "text" | "code";

export type GuessingLink = ComponentType<{ href: string; guess?: PathGuess; children: ReactNode }>;

/**
 * Turns every filename a message mentions into a link to it — the ones written
 * plainly and the ones written between backticks alike. Whether the file is
 * really there is settled when the link is drawn. A link's own text and a code
 * block are left as written.
 */
export function pathComponents(Link: GuessingLink): Pick<MarkdownComponents, "text" | "code"> {
    function PathText({ text }: { text: string }) {
        const candidates = scanPathCandidates(text);
        if (candidates.length === 0) return text;
        const out: ReactNode[] = [];
        let cursor = 0;
        for (const candidate of candidates) {
            if (candidate.start > cursor) out.push(text.slice(cursor, candidate.start));
            out.push(
                <Link key={candidate.start} href={candidate.raw} guess="text">
                    {candidate.raw}
                </Link>,
            );
            cursor = candidate.end;
        }
        if (cursor < text.length) out.push(text.slice(cursor));
        return out;
    }
    function PathCode({ text }: { text: string }) {
        if (!namesWholePath(text)) return <code>{text}</code>;
        return (
            <Link href={text} guess="code">
                {text}
            </Link>
        );
    }
    return { text: PathText, code: PathCode };
}
