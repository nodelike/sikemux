import { useEffect, useState } from "react";
import type { BoardAuto, ReviewerChoice, SettingsChanges } from "../agents/agentBoard";
import { agentBoardApi } from "../api/agentBoard";
import { modelProvidersApi } from "../api/modelProviders";
import { chatLaunchers } from "../remote/workspace";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { Switch } from "../ui/Controls";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { IconClose } from "../ui/Icons";

const MAX_SLOTS = 8;
const FIRST_THAT_WORKS = "";
const ANOTHER_CLI = "";

const SLOT_OPTIONS: DropdownOption[] = Array.from({ length: MAX_SLOTS }, (_, index) => ({
    value: String(index + 1),
    label: index === 0 ? "1 agent" : `${index + 1} agents`,
}));

const FINISH_OPTIONS: DropdownOption[] = [
    { value: "merge", label: "Merge it", detail: "Once CI, or the check command, passes" },
    { value: "pr", label: "Stop at the pull request", detail: "You review and merge it" },
];

/** A text setting that is saved when it loses focus. */
function TextSetting({
    value,
    placeholder,
    label,
    mono = false,
    onSave,
}: {
    value: string;
    placeholder: string;
    label: string;
    mono?: boolean;
    onSave: (next: string) => void;
}) {
    const [draft, setDraft] = useState<string | null>(null);
    const shown = draft ?? value;
    return (
        <input
            className={`board-auto-input${mono ? " mono" : ""}`}
            value={shown}
            placeholder={placeholder}
            aria-label={label}
            spellCheck={false}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => {
                if (draft !== null && draft.trim() !== value) onSave(draft.trim());
                setDraft(null);
            }}
            onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
            }}
        />
    );
}

/** How the project's board runs on its own, opened beside the board. */
export function AutoPanel({ folder, auto, onClose }: { folder: string; auto: BoardAuto | null; onClose: () => void }) {
    const profiles = useStore((s) => s.providerProfiles);
    const [busy, setBusy] = useState(false);
    const [payers, setPayers] = useState<DropdownOption[]>([]);
    useEffect(() => {
        void modelProvidersApi
            .list()
            .then((providers) =>
                setPayers(
                    providers
                        .filter((provider) => provider.connected && provider.agents.length > 0)
                        .map((provider) => ({ value: provider.id, label: provider.label })),
                ),
            )
            .catch(reportError("Model providers"));
    }, []);
    const settings = auto?.settings;
    const launchers: DropdownOption[] = [
        { value: FIRST_THAT_WORKS, label: "The first that works" },
        ...chatLaunchers(profiles).map((launcher) => ({ value: launcher.id, label: launcher.label })),
    ];

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
    const reviewers: DropdownOption[] = [
        { value: ANOTHER_CLI, label: "Any other signed-in agent", detail: "On another CLI than the card's" },
        ...launchers.slice(1),
    ];
    const saveReviewer = (changes: Partial<ReviewerChoice>) => {
        const launcher = changes.launcher ?? reviewer?.launcher;
        void save({ reviewer: launcher ? { launcher, provider: reviewer?.provider ?? null, model: reviewer?.model ?? null, ...changes } : null });
    };

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
                For each card you added that nobody has and nothing holds up, Auto starts an agent in a worktree and branch of its own. It plans first
                and asks its questions up front; a card it waits on you for goes to Needs you. When the agent's turn ends, Auto commits, opens a pull
                request, has an agent on another CLI review it and lands it. It keeps going while Sikemux is closed.
            </p>
            {auto?.autoNote && <p className="board-auto-note">{auto.autoNote}</p>}
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
                    <span className="board-fact-name">Agent</span>
                    <Dropdown
                        value={settings?.launcher ?? FIRST_THAT_WORKS}
                        options={launchers}
                        disabled={busy}
                        onChange={(launcher) => void save({ launcher: launcher || null })}
                    />
                </span>
                {(payers.length > 0 || settings?.provider) && (
                    <span className="board-fact">
                        <span className="board-fact-name">Paid by</span>
                        <Dropdown
                            value={settings?.provider ?? ""}
                            options={[{ value: "", label: "Its own account" }, ...payers]}
                            disabled={busy}
                            onChange={(provider) => void save({ provider: provider || null })}
                        />
                    </span>
                )}
                <span className="board-fact">
                    <span className="board-fact-name">Model</span>
                    <TextSetting
                        value={settings?.model ?? ""}
                        placeholder="The agent's own"
                        label="Model"
                        onSave={(model) => void save({ model: model || null })}
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
            </div>
            <section className="board-result">
                <span className="board-section-name">Reviewer</span>
                <div className="board-facts">
                    <span className="board-fact">
                        <span className="board-fact-name">Agent</span>
                        <Dropdown
                            value={reviewer?.launcher ?? ANOTHER_CLI}
                            options={reviewers}
                            disabled={busy}
                            onChange={(launcher) => (launcher ? saveReviewer({ launcher }) : void save({ reviewer: null }))}
                        />
                    </span>
                    {reviewer && (payers.length > 0 || reviewer.provider) && (
                        <span className="board-fact">
                            <span className="board-fact-name">Paid by</span>
                            <Dropdown
                                value={reviewer.provider ?? ""}
                                options={[{ value: "", label: "Its own account" }, ...payers]}
                                disabled={busy}
                                onChange={(provider) => saveReviewer({ provider: provider || null })}
                            />
                        </span>
                    )}
                    {reviewer && (
                        <span className="board-fact">
                            <span className="board-fact-name">Model</span>
                            <TextSetting
                                value={reviewer.model ?? ""}
                                placeholder="The agent's own"
                                label="Reviewer model"
                                onSave={(model) => saveReviewer({ model: model || null })}
                            />
                        </span>
                    )}
                </div>
                <span className="board-auto-hint">
                    Before a card merges, an agent on another CLI than the one that did it reviews the change blind, against the card and its
                    acceptance criteria. With none signed in, the pull request waits for you.
                </span>
            </section>
            <section className="board-result">
                <span className="board-section-name">Check command</span>
                <TextSetting
                    value={settings?.check ?? ""}
                    placeholder="pnpm check"
                    label="Check command"
                    mono
                    onSave={(check) => void save({ check: check || null })}
                />
                <span className="board-auto-hint">
                    Runs in the card's worktree when no CI runs on its pull request. With neither, it stops at the pull request.
                </span>
            </section>
            <section className="board-result">
                <span className="board-section-name">Jira</span>
                <div className="board-facts">
                    <span className="board-fact">
                        <span className="board-fact-name">Taken</span>
                        <TextSetting
                            value={settings?.statuses.progress ?? ""}
                            placeholder="In Progress"
                            label="Status a ticket moves to when its card is taken"
                            onSave={(progress) => progress && void save({ statuses: { progress } })}
                        />
                    </span>
                    <span className="board-fact">
                        <span className="board-fact-name">Done</span>
                        <TextSetting
                            value={settings?.statuses.done ?? ""}
                            placeholder="Done"
                            label="Status a ticket moves to when its card is done"
                            onSave={(done) => done && void save({ statuses: { done } })}
                        />
                    </span>
                </div>
                <span className="board-auto-hint">
                    Through your Jira sign-in, a ticket on the board moves with its card, gets a comment naming the pull request that landed it, and
                    closes its card when it is closed in Jira.
                </span>
            </section>
        </aside>
    );
}
