import { AgentIcon } from "@shared/ui/Icons";

/** The mark of the agent kind, in its colour, or nothing for one this page cannot draw. */
export function AgentMark({
  type,
  size = 14,
}: {
  type: string;
  size?: number;
}) {
  return (
    <AgentIcon type={type} size={size} className={`agent-glyph ${type}`} />
  );
}
