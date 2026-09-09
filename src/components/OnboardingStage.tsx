import type { AgentPresentationState } from "../state/types";
import { AgentStateIndicator } from "./AgentStateIndicator";
import { AgentIcon, IconAws, IconCommand, IconFolder } from "./Icons";

/** Region of the miniature the copy column is currently pointing at. */
export type OnboardingRegion = "rail" | "stage" | "agents";
/** Overlay the miniature is showing, mirroring the real picker and command deck. */
export type OnboardingOverlay = "sessions" | "commands";

export interface OnboardingStageProps {
    scene: number;
    region: OnboardingRegion | null;
    overlay: OnboardingOverlay | null;
    agentState: AgentPresentationState;
    commandRows: { label: string; kbd: string }[];
}

const STAGE_CAPTIONS = ["sikemux · main window", "press a binding — this reacts", "agent rail · live states", "split panes · ready to work"];

/**
 * A scale model of the Sikemux window. Every step of the tour narrates this one
 * miniature instead of an abstract diagram, so the layout the tour teaches is
 * the layout the user is about to see — same regions, same colours, same motion.
 */
export function OnboardingStage({ scene, region, overlay, agentState, commandRows }: OnboardingStageProps) {
    const split = scene === 3;
    return (
        <div className="onb-stage" data-scene={scene} data-region={region ?? "none"} aria-hidden="true">
            <div className="onb-win">
                <div className="onb-win-bar">
                    <span className="onb-win-lights">
                        <i />
                        <i />
                        <i />
                    </span>
                    <span className="onb-win-crumb">
                        <IconFolder size={9} />
                        sikemux
                        <em>/</em>
                        <b>terminal</b>
                    </span>
                    <span className="onb-win-clock">09:24</span>
                </div>

                <div className="onb-win-body">
                    <div className="onb-win-rail">
                        <span className="onb-win-group">projects</span>
                        <span className="onb-win-row is-active">
                            <IconFolder size={9} className="onb-tone-live" />
                            sikemux
                            <i className={`onb-win-pip${scene === 2 && agentState === "working" ? " is-working" : ""}`} />
                        </span>
                        <span className="onb-win-row">
                            <IconFolder size={9} className="onb-tone-live" />
                            api-gateway
                        </span>
                        <span className="onb-win-group">sessions</span>
                        <span className="onb-win-row">
                            <IconCommand size={9} className="onb-tone-cmd" />
                            command
                        </span>
                        <span className="onb-win-row">
                            <IconAws size={9} className="onb-tone-warn" />
                            billing
                        </span>
                    </div>

                    <div className="onb-win-agents">
                        <span className="onb-win-group">agents</span>
                        <span className="onb-win-row is-active">
                            <AgentIcon type="claude" size={9} className="onb-tone-claude" />
                            refactor
                            <AgentStateIndicator state={agentState} />
                        </span>
                        <span className="onb-win-row">
                            <AgentIcon type="codex" size={9} className="onb-tone-codex" />
                            tests
                            <AgentStateIndicator state="done" />
                        </span>
                    </div>

                    <div className="onb-win-main">
                        <div className="onb-win-tabs">
                            <span className="is-active">terminal</span>
                            <span>git</span>
                            {split && <span>editor</span>}
                        </div>
                        <div className={`onb-win-panes${split ? " is-split" : ""}`}>
                            <div className="onb-win-pane">
                                <span className="onb-win-line">
                                    <b>$</b> pnpm tauri dev
                                </span>
                                <span className="onb-win-line is-dim">▸ ready in 412 ms</span>
                                <span className="onb-win-line">
                                    <b>$</b> git status -sb
                                </span>
                                <span className="onb-win-line is-dim">## main…origin/main</span>
                                <span className="onb-win-line">
                                    <b>$</b> <i className="onb-win-caret" />
                                </span>
                            </div>
                            {split && (
                                <div className="onb-win-pane is-code">
                                    <span className="onb-win-code" style={{ width: "72%" }} />
                                    <span className="onb-win-code is-acc" style={{ width: "48%" }} />
                                    <span className="onb-win-code" style={{ width: "84%" }} />
                                    <span className="onb-win-code is-live" style={{ width: "36%" }} />
                                    <span className="onb-win-code" style={{ width: "62%" }} />
                                </div>
                            )}
                        </div>
                    </div>

                    {overlay && (
                        <div className="onb-win-scrim">
                            <div className="onb-win-overlay" key={overlay}>
                                <span className="onb-win-overlay-head">{overlay === "sessions" ? "open session" : "command deck"}</span>
                                {overlay === "sessions" ? (
                                    <>
                                        <span className="onb-win-overlay-row is-sel">
                                            <IconFolder size={9} className="onb-tone-live" />
                                            sikemux<em>~/proj</em>
                                        </span>
                                        <span className="onb-win-overlay-row">
                                            <IconAws size={9} className="onb-tone-warn" />
                                            billing<em>prod</em>
                                        </span>
                                        <span className="onb-win-overlay-row">
                                            <IconCommand size={9} className="onb-tone-cmd" />
                                            command<em>zsh</em>
                                        </span>
                                    </>
                                ) : (
                                    commandRows.map((row, index) => (
                                        <span key={row.label} className={`onb-win-overlay-row${index === 0 ? " is-sel" : ""}`}>
                                            {row.label}
                                            <em>{row.kbd}</em>
                                        </span>
                                    ))
                                )}
                            </div>
                        </div>
                    )}
                </div>
            </div>
            <span className="onb-stage-caption">{STAGE_CAPTIONS[scene] ?? STAGE_CAPTIONS[0]}</span>
        </div>
    );
}
