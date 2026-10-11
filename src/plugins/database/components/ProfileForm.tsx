import { useRef, useState } from "react";
import { confirmDialog, pickFile, swallow } from "../../../plugin-api/host";
import { Checkbox, Dropdown } from "../../../plugin-api/ui";
import { databaseApi, failureMessage, type DatabaseProfile, type ProfileDraft, type Tested, type TlsMode } from "../api";
import { ENGINES, TLS_MODES, defaultPort, isServer, missingField, parsePort, withEngine } from "../profileForm";

const SQLITE_FILES = { name: "SQLite database", extensions: ["db", "sqlite", "sqlite3", "db3"] };

type Outcome = { kind: "tested"; tested: Tested } | { kind: "failed"; message: string } | null;

export function ProfileForm({
    initial,
    saved,
    onSaved,
    onRemoved,
    onCancel,
}: {
    initial: ProfileDraft;
    /** The profile as saved, when this edits one. */
    saved: DatabaseProfile | null;
    onSaved: (profile: DatabaseProfile) => void;
    onRemoved: () => void;
    onCancel: () => void;
}) {
    const [draft, setDraft] = useState(initial);
    const [password, setPassword] = useState("");
    const [forgetPassword, setForgetPassword] = useState(false);
    const [busy, setBusy] = useState<"test" | "save" | null>(null);
    const [outcome, setOutcome] = useState<Outcome>(null);
    const working = useRef(false);

    const missing = missingField(draft);
    const change = (next: ProfileDraft) => {
        setDraft(next);
        setOutcome(null);
    };
    /** Typed text replaces the saved password; ticking "forget" clears it; otherwise the saved one stays. */
    const passwordToSend = () => (forgetPassword ? "" : password || undefined);

    const run = async (kind: "test" | "save") => {
        if (working.current || missing) return;
        working.current = true;
        setBusy(kind);
        setOutcome(null);
        try {
            if (kind === "test") setOutcome({ kind: "tested", tested: await databaseApi.test(draft, passwordToSend()) });
            else onSaved(await databaseApi.save(draft, passwordToSend()));
        } catch (failure) {
            setOutcome({ kind: "failed", message: failureMessage(failure) });
        } finally {
            working.current = false;
            setBusy(null);
        }
    };

    const remove = async () => {
        if (!saved) return;
        const confirmed = await confirmDialog({
            title: `Remove ${saved.name}?`,
            body: "Sikemux forgets this connection and its saved password. The database itself is not touched.",
            confirmLabel: "Remove",
            destructive: true,
        });
        if (!confirmed) return;
        await databaseApi.remove(saved.id);
        onRemoved();
    };

    const choose = async () => {
        const path = await pickFile("Choose a SQLite database", SQLITE_FILES);
        if (path && draft.engine === "sqlite") change({ ...draft, path, name: draft.name || (path.split("/").pop() ?? "") });
    };

    return (
        <form
            className="db-form"
            aria-label={saved ? `Edit ${saved.name}` : "New connection"}
            onSubmit={(event) => {
                event.preventDefault();
                void run("save");
            }}>
            <h2>{saved ? saved.name : "New connection"}</h2>
            <div className="db-segmented" role="radiogroup" aria-label="Engine">
                {ENGINES.map((engine) => (
                    <button
                        key={engine.value}
                        type="button"
                        role="radio"
                        aria-checked={draft.engine === engine.value}
                        className={draft.engine === engine.value ? "active" : undefined}
                        onClick={() => change(withEngine(draft, engine.value))}>
                        {engine.label}
                    </button>
                ))}
            </div>
            <label className="db-field">
                <span>Name</span>
                <input
                    value={draft.name}
                    onChange={(event) => change({ ...draft, name: event.target.value })}
                    placeholder={isServer(draft) ? "Production" : "Local app data"}
                    autoFocus
                    spellCheck={false}
                />
            </label>
            {isServer(draft) ? (
                <>
                    <div className="db-inline">
                        <label className="db-field db-grow">
                            <span>Host</span>
                            <input
                                value={draft.host}
                                onChange={(event) => change({ ...draft, host: event.target.value })}
                                placeholder="localhost"
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                            />
                        </label>
                        <label className="db-field port">
                            <span>Port</span>
                            <input
                                value={draft.port ?? ""}
                                onChange={(event) => change({ ...draft, port: parsePort(event.target.value) })}
                                placeholder={String(defaultPort(draft.engine))}
                                inputMode="numeric"
                            />
                        </label>
                    </div>
                    <label className="db-field">
                        <span>Database</span>
                        <input
                            value={draft.database}
                            onChange={(event) => change({ ...draft, database: event.target.value })}
                            placeholder={draft.engine === "mysql" ? "Optional; pick a schema once connected" : "Same as the user name"}
                            spellCheck={false}
                            autoCapitalize="off"
                        />
                    </label>
                    <div className="db-inline">
                        <label className="db-field db-grow">
                            <span>User</span>
                            <input
                                value={draft.user}
                                onChange={(event) => change({ ...draft, user: event.target.value })}
                                placeholder={draft.engine === "mysql" ? "root" : "postgres"}
                                spellCheck={false}
                                autoCapitalize="off"
                                autoCorrect="off"
                            />
                        </label>
                        <label className="db-field db-grow">
                            <span>Password</span>
                            <input
                                type="password"
                                value={password}
                                disabled={forgetPassword}
                                onChange={(event) => {
                                    setPassword(event.target.value);
                                    setOutcome(null);
                                }}
                                placeholder={saved?.hasPassword ? "Saved in the Keychain" : "None"}
                            />
                        </label>
                    </div>
                    {saved?.hasPassword && (
                        <Checkbox checked={forgetPassword} onChange={setForgetPassword}>
                            Forget the saved password
                        </Checkbox>
                    )}
                    <div className="db-field">
                        <span>Encryption</span>
                        <div className="db-inline">
                            <Dropdown
                                label="Encryption"
                                value={draft.tls}
                                options={TLS_MODES.map((mode) => ({ value: mode.value, label: mode.label, detail: mode.detail }))}
                                onChange={(tls) => change({ ...draft, tls: tls as TlsMode })}
                            />
                            <span className="db-hint">{TLS_MODES.find((mode) => mode.value === draft.tls)?.detail}</span>
                        </div>
                    </div>
                </>
            ) : (
                <label className="db-field">
                    <span>Database file</span>
                    <span className="db-inline">
                        <input
                            className="db-grow"
                            value={draft.path}
                            onChange={(event) => change({ ...draft, path: event.target.value })}
                            placeholder="~/data/app.db"
                            spellCheck={false}
                        />
                        <button type="button" className="db-button" onClick={() => void choose().catch(swallow("choose a database file"))}>
                            Choose…
                        </button>
                    </span>
                </label>
            )}
            <Checkbox
                checked={draft.readOnly}
                onChange={(readOnly) => change({ ...draft, readOnly, agentWrites: readOnly ? false : draft.agentWrites })}>
                Read only: open connections read-only, a guard against accidental changes
            </Checkbox>
            <Checkbox checked={draft.agentWrites} disabled={draft.readOnly} onChange={(agentWrites) => change({ ...draft, agentWrites })}>
                Let agents change data here. Otherwise their queries can only read
            </Checkbox>
            {outcome?.kind === "tested" && (
                <div className="db-callout" data-tone="ok" role="status">
                    Connected to {outcome.tested.version} in {outcome.tested.millis} ms
                </div>
            )}
            {outcome?.kind === "failed" && (
                <div className="db-callout" data-tone="danger" role="alert">
                    {outcome.message}
                </div>
            )}
            {missing && <div className="db-hint">Needs {missing}.</div>}
            <div className="db-actions">
                {saved && (
                    <button type="button" className="db-button danger" onClick={() => void remove().catch(swallow("remove the connection"))}>
                        Remove
                    </button>
                )}
                <span className="db-grow" />
                <button type="button" className="db-button" onClick={onCancel}>
                    Cancel
                </button>
                <button type="button" className="db-button" disabled={!!missing || busy !== null} onClick={() => void run("test")}>
                    {busy === "test" ? "Connecting…" : "Test"}
                </button>
                <button type="submit" className="db-button primary" disabled={!!missing || busy !== null}>
                    {busy === "save" ? "Saving…" : "Save"}
                </button>
            </div>
        </form>
    );
}
