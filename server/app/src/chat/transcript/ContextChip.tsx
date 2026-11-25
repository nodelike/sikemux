import {
  IconFile,
  IconIssue,
  IconPresentation,
  IconPullRequest,
} from "@shared/ui/Icons";
import { contextChip, type ContextChip } from "@shared/chat/promptContext";
import type { SentContext } from "@shared/wire.generated";

function ContextMark({
  kind,
  size,
}: {
  kind: ContextChip["kind"];
  size: number;
}) {
  if (kind === "issue") return <IconIssue size={size} />;
  if (kind === "pull") return <IconPullRequest size={size} />;
  if (kind === "artifact") return <IconPresentation size={size} />;
  return <IconFile size={size} />;
}

/** An issue, pull request or other context item, named by its number and title rather than shown whole. */
export function ContextChipLabel({
  item,
  size = 12,
}: {
  item: SentContext;
  size?: number;
}) {
  const chip = contextChip(item);
  return (
    <>
      <span className="chat-context-mark" data-kind={chip.kind ?? "other"}>
        <ContextMark kind={chip.kind} size={size} />
      </span>
      {chip.number && (
        <span className="chat-context-number">#{chip.number}</span>
      )}
      <span className="chat-context-title">{chip.title}</span>
    </>
  );
}
