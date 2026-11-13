import { IconReply } from "@shared/ui/Icons";
import {
  FROM_NOW_ON,
  OVERRULED,
  OVERRULED_UNDONE,
  quoted,
  STANDS_AGAIN,
  STOPPED_AUTO,
  weekCame,
  weekCount,
} from "@shared/calls/words";
import type { ChiefWeek, OverruledDecision } from "@shared/wire.generated";

/** The quiet line an Overrule leaves in the chief's chat, as the Mac words it, without Undo: a viewer acts on nothing. */
export function OverruledLine({ overruled }: { overruled: OverruledDecision }) {
  const why = overruled.undone
    ? STANDS_AGAIN
    : overruled.stoppedAuto
      ? STOPPED_AUTO
      : overruled.rule !== undefined
        ? FROM_NOW_ON
        : undefined;
  return (
    <div className="chat-chief-line">
      <div className="chat-chief-head">
        <IconReply size={12} />
        <span>{overruled.undone ? OVERRULED_UNDONE : OVERRULED}</span>
      </div>
      {overruled.text && (
        <div className="chat-chief-quoted">{quoted(overruled.text)}</div>
      )}
      {why && <div className="chat-chief-why">{why}</div>}
    </div>
  );
}

/** The chief's count of last week, in its own words under the day and time it came. */
export function WeekLine({ week }: { week: ChiefWeek }) {
  return (
    <div className="chat-chief-week">
      <div className="chat-chief-when">{weekCame(week.to)}</div>
      <div className="chat-chief-count">{weekCount(week)}</div>
    </div>
  );
}
