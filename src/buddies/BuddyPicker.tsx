import { useEffect, useRef, useState } from "react";
import { useModalFocus } from "../hooks/useModalFocus";
import { useOccludeNativeViews } from "../state/nativeViews";
import { useStore } from "../state/store";
import type { Agent } from "../state/types";
import { AgentBuddy, BuddyArt } from "./Buddy";
import { BUDDY_PACKS, resolveBuddy, type BuddyShape } from "@shared/buddies/catalogue";
import { closeBuddyPicker, resetAgentBuddy, setAgentBuddyColor, setAgentBuddyShape, useBuddyPicker } from "./commands";

export function BuddyPickerHost() {
    const agentId = useBuddyPicker((s) => s.agentId);
    const agent = useStore((s) => (agentId ? s.agents[agentId] : undefined));
    useOccludeNativeViews(agent !== undefined);
    if (!agent) return null;
    return <BuddyPicker key={agent.id} agent={agent} />;
}

function BuddyPicker({ agent }: { agent: Agent }) {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const style = useStore((s) => s.faceStyle);
    const look = resolveBuddy(agent, style);
    const [packId, setPackId] = useState(look.pack.id);
    const [hovered, setHovered] = useState<BuddyShape | null>(null);
    const pack = BUDDY_PACKS.find((candidate) => candidate.id === packId) ?? look.pack;
    const colorIndex = look.colorIndex % pack.colors.length;
    const color = pack.colors[colorIndex];
    const chosen = agent.buddy?.pack !== undefined;
    const named = hovered ? `${pack.name} · ${hovered.name}` : `${look.pack.name} · ${look.shape.name}`;

    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            closeBuddyPicker();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, []);

    return (
        <div className="dlg-scrim" onMouseDown={closeBuddyPicker}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="dlg buddy-dlg"
                role="dialog"
                aria-modal="true"
                aria-label={`Buddy for ${agent.title}`}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="buddy-head">
                    <AgentBuddy agent={agent} size={44} />
                    <div className="buddy-head-text">
                        <h2 className="dlg-title">{agent.name ?? "Buddy"}</h2>
                        <span className="buddy-head-name">{named}</span>
                    </div>
                    <div className="buddy-colors" role="radiogroup" aria-label="Colour">
                        {pack.colors.map((swatch, index) => (
                            <button
                                key={swatch}
                                type="button"
                                role="radio"
                                aria-checked={index === colorIndex}
                                aria-label={`Colour ${index + 1}`}
                                className={`buddy-swatch${index === colorIndex ? " on" : ""}`}
                                onClick={() => setAgentBuddyColor(agent.id, index)}>
                                <span style={{ background: swatch }} />
                            </button>
                        ))}
                    </div>
                </div>
                <div className="buddy-body">
                    <div className="buddy-packs" role="tablist" aria-label="Packs" aria-orientation="vertical">
                        {BUDDY_PACKS.map((candidate) => (
                            <button
                                key={candidate.id}
                                type="button"
                                role="tab"
                                aria-selected={candidate.id === pack.id}
                                className={`buddy-pack${candidate.id === pack.id ? " on" : ""}`}
                                onClick={() => setPackId(candidate.id)}>
                                <BuddyArt
                                    pack={candidate}
                                    shape={candidate.shapes[0]}
                                    color={candidate.colors[look.colorIndex % candidate.colors.length]}
                                    state="idle"
                                    size={18}
                                    className="still"
                                />
                                <span>{candidate.name}</span>
                                {look.pack.id === candidate.id && <i className="buddy-pack-dot" aria-hidden="true" />}
                            </button>
                        ))}
                    </div>
                    <div className="buddy-grid" role="listbox" aria-label={pack.name} onMouseLeave={() => setHovered(null)}>
                        {pack.shapes.map((shape) => {
                            const on = look.pack.id === pack.id && look.shape.id === shape.id;
                            return (
                                <button
                                    key={shape.id}
                                    type="button"
                                    role="option"
                                    aria-selected={on}
                                    aria-label={shape.name}
                                    className={`buddy-tile${on ? " on" : ""}`}
                                    onMouseEnter={() => setHovered(shape)}
                                    onClick={() => setAgentBuddyShape(agent.id, pack.id, shape.id)}>
                                    <BuddyArt pack={pack} shape={shape} color={color} state="idle" size={40} />
                                </button>
                            );
                        })}
                    </div>
                </div>
                <div className="dlg-foot">
                    <span className="dlg-hint">{chosen ? "Picked for this agent" : "Following the default in Settings"}</span>
                    {chosen && (
                        <button type="button" className="dlg-btn" onClick={() => resetAgentBuddy(agent.id)}>
                            Use Default
                        </button>
                    )}
                    <button type="button" className="dlg-btn primary" onClick={closeBuddyPicker}>
                        Done
                    </button>
                </div>
            </div>
        </div>
    );
}
