import { useState } from "react";
import { useNow } from "./hooks/useNow";
import { secondsSince } from "@shared/chat/toolView";

/* Counts up in whole seconds like the Thinking row does, on a clock of its own
   so the tick redraws this label and nothing around it. */
export function LiveSeconds({
  since,
  spent = 0,
}: {
  since?: number;
  spent?: number;
}) {
  const [started] = useState(() => since ?? Date.now());
  const now = useNow();
  return (
    <span className="chat-tool-elapsed" aria-hidden="true">
      {secondsSince(started, now, spent)}
    </span>
  );
}
