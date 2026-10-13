import { useEffect, useState } from "react";
import { secondsSince } from "./toolView";

/* Counts up in whole seconds like the Thinking row does, on a clock of its own
   so the tick redraws this label and nothing around it. */
export function LiveSeconds({ since, spent = 0 }: { since?: number; spent?: number }) {
    const [started] = useState(() => since ?? Date.now());
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <span className="chat-tool-elapsed" aria-hidden="true">
            {secondsSince(started, now, spent)}
        </span>
    );
}
