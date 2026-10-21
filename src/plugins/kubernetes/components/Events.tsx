import { useState } from "react";
import { Seg } from "../../../plugin-api/ui";
import type { KubeEvent } from "../api";
import { EventList } from "./PodView";

type Which = "all" | "warnings";

export function Events({ events, find, onOpenPod }: { events: KubeEvent[]; find: string; onOpenPod: (event: KubeEvent) => void }) {
    const [which, setWhich] = useState<Which>("warnings");
    const words = find.trim().toLowerCase();
    const found = events.filter(
        (event) =>
            !words ||
            event.object.toLowerCase().includes(words) ||
            event.message.toLowerCase().includes(words) ||
            event.reason.toLowerCase().includes(words),
    );
    const warnings = found.filter((event) => event.kind === "Warning");
    return (
        <section className="k8s-section" aria-label="Events">
            <div className="k8s-toolbar">
                <Seg
                    label="Show"
                    value={which}
                    onChange={setWhich}
                    options={[
                        { value: "warnings", label: "Warnings", count: warnings.length },
                        { value: "all", label: "All", count: found.length },
                    ]}
                />
            </div>
            <EventList
                events={which === "warnings" ? warnings : found}
                showObject
                onOpenPod={(name) => {
                    const event = events.find((each) => each.object === `Pod/${name}`);
                    if (event) onOpenPod(event);
                }}
                empty={which === "warnings" ? "No warnings. Everything here is going as asked." : "No events in the last hour."}
            />
        </section>
    );
}
