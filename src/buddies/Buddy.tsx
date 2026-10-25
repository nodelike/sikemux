import { memo, useMemo, useState, type CSSProperties } from "react";
import { useStore } from "../state/store";
import { AgentIcon } from "@shared/ui/Icons";
import type { Agent } from "../state/types";
import { buddyStateOfGroup, type BuddyPack, type BuddyShape, type BuddyState } from "@shared/buddies/catalogue";
import { drawnFace } from "@shared/buddies/faces";
import { bodyMotion, buddyBox, buddyMarkup } from "@shared/buddies/render";
import "./buddies.css";

let nextUid = 0;

export const BuddyArt = memo(function BuddyArt({
    pack,
    shape,
    color,
    state,
    size,
    className,
    box: measured,
}: {
    pack: BuddyPack;
    shape: BuddyShape;
    color: string;
    state: BuddyState;
    size: number;
    className?: string;
    /** The part of the shape's square it fills, when the core already says. */
    box?: string;
}) {
    const [uid] = useState(() => `bd${++nextUid}`);
    const still = !useStore((s) => s.faceStyle.motion);
    const box = useMemo(() => measured ?? buddyBox(pack, shape), [measured, pack, shape]);
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
    const face = useStore((s) => s.faces[agent.id]);
    const style = useStore((s) => s.faceStyle);
    const look = useMemo(() => drawnFace(agent, face, style), [agent, face, style]);
    return (
        <BuddyArt
            pack={look.pack}
            shape={look.shape}
            color={look.color}
            box={look.box}
            state={state ?? buddyStateOfGroup(live)}
            size={size}
            className={className}
        />
    );
});

/** The agent's buddy when buddies are on, otherwise its harness logo, each in its own wrapper. */
export function AgentFace({
    agent,
    size,
    logoSize,
    logoClass,
    buddyClass,
}: {
    agent: Pick<Agent, "id" | "type" | "buddy">;
    size: number;
    logoSize: number;
    logoClass: string;
    buddyClass: string;
}) {
    const face = useStore((s) => s.faces[agent.id]);
    const on = useStore((s) => s.faceStyle.on);
    const wears = face === undefined ? on : face !== null;
    return wears ? (
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
    const on = useStore((s) => s.faceStyle.on);
    if (!on) return null;
    return (
        <div className="composer-buddy" aria-hidden="true">
            <AgentBuddy agent={agent} size={40} />
        </div>
    );
}
