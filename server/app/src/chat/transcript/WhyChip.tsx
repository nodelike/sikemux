import {
  IconBook,
  IconCheck,
  IconCommit,
  IconMerge,
  IconMessage,
} from "@shared/ui/Icons";
import { whyWords, type WhyIcon } from "@shared/chat/why";
import type { WhyChip } from "@shared/wire.generated";

const ICONS: Record<WhyIcon, typeof IconMessage> = {
  IconMessage,
  IconCommit,
  IconCheck,
  IconBook,
  IconMerge,
};

/** What a claim in an answer rests on, as the core checked it: what it is, then when. A viewer of a share opens nothing from it. */
export function WhyChipLabel({ chip }: { chip: WhyChip }) {
  const { icon, key, title } = whyWords(chip);
  const Mark = ICONS[icon];
  return (
    <span className="chat-why-chip">
      <Mark size={12} />
      <span className="chat-why-key">{key}</span>
      {title && <span className="chat-why-title">{title}</span>}
    </span>
  );
}
