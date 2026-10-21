import { copyWithNotice } from "../../../plugin-api/host";
import { Seg } from "../../../plugin-api/ui";
import type { ActivityEvent } from "../api";
import { ago, eventTone } from "../format";
import { Dot } from "./parts";

const HOURS: { value: string; label: string }[] = [
    { value: "1", label: "1h" },
    { value: "24", label: "24h" },
    { value: "168", label: "7d" },
    { value: "720", label: "30d" },
];

export function Activity({
    events,
    hours,
    failedOnly,
    showGroup,
    onWindow,
}: {
    events: ActivityEvent[];
    hours: number;
    failedOnly: boolean;
    showGroup: boolean;
    onWindow: (change: { hours?: number; failedOnly?: boolean }) => void;
}) {
    return (
        <section className="az-section" aria-label="Activity">
            <div className="az-toolbar">
                <Seg label="Since" value={String(hours)} onChange={(value) => onWindow({ hours: Number(value) })} options={HOURS} />
                <Seg
                    label="Show"
                    value={failedOnly ? "failed" : "all"}
                    onChange={(value) => onWindow({ failedOnly: value === "failed" })}
                    options={[
                        { value: "all", label: "Everything" },
                        { value: "failed", label: "Only what failed" },
                    ]}
                />
            </div>
            {events.length === 0 ? (
                <p className="az-note">{failedOnly ? "Nothing failed in this window." : "Nothing has happened in this window."}</p>
            ) : (
                <ul className="az-events" aria-label="Activity log">
                    {events.map((event, index) => (
                        <li key={`${event.at}-${index}`}>
                            <Dot tone={eventTone(event)} label={event.status || event.level} />
                            <span className="az-event-main">
                                <span className="az-event-operation">{event.operation}</span>
                                <span className="az-event-meta">
                                    <button
                                        type="button"
                                        className="az-link"
                                        title="Copy the name"
                                        onClick={() => copyWithNotice(event.resource, "The resource name")}>
                                        {event.resource}
                                    </button>
                                    {showGroup && event.group ? <span className="az-muted">{event.group}</span> : null}
                                    {event.who ? <span className="az-muted">{event.who}</span> : null}
                                </span>
                                {event.message ? <span className="az-event-message">{event.message}</span> : null}
                            </span>
                            <span className="az-event-status">{event.status}</span>
                            <span className="az-muted az-event-when">{ago(event.at)} ago</span>
                        </li>
                    ))}
                </ul>
            )}
        </section>
    );
}
