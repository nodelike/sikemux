import { useState } from "react";
import type { BoardAuto, SettingsChanges } from "../agents/agentBoard";
import { type ChatAgentType } from "../agents/catalog";
import { agentBoardApi } from "../api/agentBoard";
import { LaunchPicker, type LaunchChoice } from "../chat/LaunchPicker";
import { launcherId } from "../remote/workspace";
import * as cmd from "../state/commands";
import { reportError } from "../state/toast";
import { Switch } from "../ui/Controls";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { IconClose } from "../ui/Icons";

const MAX_SLOTS = 8;

const SLOT_OPTIONS: DropdownOption[] = Array.from({ length: MAX_SLOTS }, (_, index) => ({
    value: String(index + 1),
    label: index === 0 ? "1 agent" : `${index + 1} agents`,
}));

const FINISH_OPTIONS: DropdownOption[] = [
    { value: "merge", label: "Merge it", detail: "Once CI, or the check command, passes" },
    { value: "pr", label: "Stop at the pull request", detail: "You review and merge it" },
];

/** A launcher as the core names it, `claude` or `claude:<profile>`, as the picker holds it. */
function choiceOf(launcher: string | null | undefined, provider: string | null, model: string | null): LaunchChoice | null {
    if (!launcher) return null;
    const [type, profileId] = launcher.split(":");
    return { type: type as ChatAgentType, profileId, provider, model };
}

/** How the project's board runs on its own, opened beside the board. */
export function AutoPanel({ folder, auto, onClose }: { folder: string; auto: BoardAuto | null; onClose: () => void }) {
    const [busy, setBusy] = useState(false);
    const settings = auto?.settings;

    const save = async (changes: SettingsChanges) => {
        setBusy(true);
        try {
            cmd.setAgentBoard(folder, await agentBoardApi.settings(folder, changes));
        } catch (error) {
            reportError("change Auto")(error);
        } finally {
            setBusy(false);
        }
    };

    const reviewer = settings?.reviewer ?? null;

    return (
        <aside className="board-card-panel" aria-label="Auto">
            <header className="board-panel-head">
                <span className="board-panel-name">Auto</span>
                <span className="kanban-grow" />
                <button type="button" className="kanban-icon-button" title="Close" aria-label="Close Auto" onClick={onClose}>
                    <IconClose size={12} />
                </button>
            </header>
            <label className="board-auto-row">
                <Switch checked={settings?.auto ?? false} disabled={busy} onChange={(on) => void save({ auto: on })} label="Auto" />
                <span>Start agents on cards by itself</span>
            </label>
            <p className="board-auto-about">
                Starts agents on your open cards, each on its own branch; another agent reviews the work before it lands.
            </p>
            {auto?.autoNote && <p className="board-auto-note">{auto.autoNote}</p>}
            <section className="board-section">
                <div className="board-facts">
                    <span className="board-fact">
                        <span className="board-fact-name">At once</span>
                        <Dropdown
                            value={String(settings?.slots ?? 2)}
                            options={SLOT_OPTIONS}
                            disabled={busy}
                            onChange={(slots) => void save({ slots: Number(slots) })}
                        />
                    </span>
                    <span className="board-fact">
                        <span className="board-fact-name">Then</span>
                        <Dropdown
                            value={settings?.finish ?? "merge"}
                            options={FINISH_OPTIONS}
                            disabled={busy}
                            onChange={(finish) => void save({ finish: finish as "merge" | "pr" })}
                        />
                    </span>
                    <span className="board-fact">
                        <span className="board-fact-name">Agent</span>
                        <LaunchPicker
                            name="Agent"
                            any="The first that works"
                            disabled={busy}
                            value={choiceOf(settings?.launcher, settings?.provider ?? null, settings?.model ?? null)}
                            onChange={(next) =>
                                void save({
                                    launcher: next ? launcherId(next.type, next.profileId) : null,
                                    provider: next?.provider ?? null,
                                    model: next?.model ?? null,
                                })
                            }
                        />
                    </span>
                    <span className="board-fact">
                        <span className="board-fact-name">Reviewer</span>
                        <LaunchPicker
                            name="Reviewer"
                            any="Any other agent"
                            disabled={busy}
                            value={choiceOf(reviewer?.launcher, reviewer?.provider ?? null, reviewer?.model ?? null)}
                            onChange={(next) =>
                                void save({
                                    reviewer: next
                                        ? { launcher: launcherId(next.type, next.profileId), provider: next.provider, model: next.model }
                                        : null,
                                })
                            }
                        />
                    </span>
                </div>
                <span className="board-auto-hint">The reviewer checks each change against its card before it merges.</span>
            </section>
        </aside>
    );
}
