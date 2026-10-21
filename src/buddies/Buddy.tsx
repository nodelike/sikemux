import { memo, useMemo, useState, type CSSProperties } from "react";
import { useStore } from "../state/store";
import { AgentIcon } from "../ui/Icons";
import type { Agent } from "../state/types";
import { buddyStateOfGroup, resolveBuddy, type BuddyPack, type BuddyShape, type BuddyState } from "./catalogue";
import { wearsBuddy } from "./published";
import { bodyMotion, buddyBox, buddyMarkup } from "./render";
import "./buddies.css";

let nextUid = 0;

export const BuddyArt = memo(function BuddyArt({
    pack,
    shape,
    color,
    state,
    size,
    className,
}: {
    pack: BuddyPack;
    shape: BuddyShape;
    color: string;
    state: BuddyState;
    size: number;
    className?: string;
}) {
    const [uid] = useState(() => `bd${++nextUid}`);
    const still = !useStore((s) => s.buddyMotion);
    const box = useMemo(() => buddyBox(pack, shape), [pack, shape]);
    const html = useMemo(() => buddyMarkup(pack, shape, color, state, uid, box), [pack, shape, color, state, uid, box]);
    const motion = bodyMotion(state);
    return (
        <span
            className={`buddy${still ? " still" : ""}${className ? ` ${className}` : ""}`}
            data-motion={motion || undefined}
            style={{ width: size, height: size, "--bd-side": box.split(" ")[2] } as CSSProperties}
            dangerouslySetInnerHTML={{ __html: html }}
        />
    );
});

/** An agent's character, showing what the agent is doing unless `state` says otherwise. */
export const AgentBuddy = memo(function AgentBuddy({
    agent,
    size,
    state,
    className,
}: {
    agent: Pick<Agent, "id" | "buddy">;
    size: number;
    state?: BuddyState;
    className?: string;
}) {
    const live = useStore((s) => (state ? undefined : s.agentStatus[agent.id]?.group));
    const pack = useStore((s) => s.buddyPack);
    const shape = useStore((s) => s.buddyShape);
    const look = useMemo(() => resolveBuddy(agent, { pack, shape }), [agent, pack, shape]);
    return (
        <BuddyArt pack={look.pack} shape={look.shape} color={look.color} state={state ?? buddyStateOfGroup(live)} size={size} className={className} />
    );
});

/** The agent's buddy when buddies are on or the person named it, otherwise its harness logo, each in its own wrapper. */
export function AgentFace({
    agent,
    size,
    logoSize,
    logoClass,
    buddyClass,
}: {
    agent: Pick<Agent, "id" | "type" | "buddy" | "name">;
    size: number;
    logoSize: number;
    logoClass: string;
    buddyClass: string;
}) {
    const on = wearsBuddy(
        useStore((s) => s.buddies),
        !!agent.name,
    );
    return on ? (
        <span className={buddyClass}>
            <AgentBuddy agent={agent} size={size} />
        </span>
    ) : (
        <span className={logoClass}>
            <AgentIcon type={agent.type} size={logoSize} />
        </span>
    );
}

/** The agent's buddy peeking over the top of the composer, or of whatever is docked on it. */
export function ComposerBuddy({ agent }: { agent: Pick<Agent, "id" | "buddy"> }) {
    const on = useStore((s) => s.buddies);
    if (!on) return null;
    return (
        <div className="composer-buddy" aria-hidden="true">
            <AgentBuddy agent={agent} size={40} />
        </div>
    );
}
