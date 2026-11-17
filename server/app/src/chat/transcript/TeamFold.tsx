import { AgentMark } from "../agents/AgentMark";
import { IconChevron } from "@shared/ui/Icons";
import { teamCount, teamTook, teamWords } from "@shared/chat/team";
import type { TeamLine } from "@shared/wire.generated";

/** The one line a run of messages between agents folds to, which opens where it is. */
export function TeamFold({
  line,
  open,
  all,
  onToggle,
  onAll,
}: {
  line: TeamLine;
  open: boolean;
  all: boolean;
  onToggle: () => void;
  onAll: () => void;
}) {
  const { words, wanting } = teamWords(line);
  return (
    <div className="chat-team">
      <button
        type="button"
        className="chat-team-row"
        aria-expanded={open}
        onClick={onToggle}
      >
        <IconChevron size={10} className="chat-tools-chevron" />
        <span className="chat-team-marks" aria-hidden="true">
          {line.agents.map((agent) => (
            <span
              key={agent.agentId ?? agent.cli}
              className={`agent-glyph ${agent.cli}`}
            >
              <AgentMark type={agent.cli} size={16} />
            </span>
          ))}
          {line.more > 0 && (
            <span className="chat-team-more">+{line.more}</span>
          )}
        </span>
        <span className="chat-team-count">{teamCount(line.messages)}</span>
        <span
          className="chat-team-words"
          data-wanting={wanting ? "" : undefined}
        >
          {words}
        </span>
        {line.took !== undefined && (
          <span className="chat-team-took">{teamTook(line.took)}</span>
        )}
      </button>
      {open && !all && line.earlier > 0 && (
        <button type="button" className="chat-team-earlier" onClick={onAll}>
          Show {line.earlier} earlier
        </button>
      )}
    </div>
  );
}
