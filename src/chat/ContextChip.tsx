import { IconFile, IconIssue, IconPullRequest } from "../ui/Icons";
import { contextChip, type SentContext } from "./promptContext";

export function ContextMark({ kind, size }: { kind: "issue" | "pull" | null; size: number }) {
    if (kind === "issue") return <IconIssue size={size} />;
    if (kind === "pull") return <IconPullRequest size={size} />;
    return <IconFile size={size} />;
}

/** An issue, pull request or other context item, named by its number and title rather than shown whole. */
export function ContextChipLabel({ item, size = 12 }: { item: SentContext; size?: number }) {
    const chip = contextChip(item);
    return (
        <>
            <span className="chat-context-mark" data-kind={chip.kind ?? "other"}>
                <ContextMark kind={chip.kind} size={size} />
            </span>
            {chip.number && <span className="chat-context-number">#{chip.number}</span>}
            <span className="chat-context-title">{chip.title}</span>
        </>
    );
}
