import { useState } from "react";
import { databaseApi, failureMessage, refreshDatabase, type Connected, type DatabaseProfile } from "../api";
import { defaultPort, engineLabel, isServer } from "../profileForm";

/** A saved connection that is not open: where it points, and a button to connect. */
export function ProfileDetail({ profile, connected, onEdit }: { profile: DatabaseProfile; connected: Connected | null; onEdit: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const toggle = async () => {
        setBusy(true);
        setError(null);
        try {
            if (connected) await databaseApi.disconnect(profile.id);
            else await databaseApi.connect(profile.id);
            refreshDatabase();
        } catch (failure) {
            setError(failureMessage(failure));
        } finally {
            setBusy(false);
        }
    };

    return (
        <article className="db-detail" aria-label={profile.name}>
            <header className="db-detail-head">
                <h2>{profile.name}</h2>
                {profile.readOnly && <span className="db-badge">Read only</span>}
            </header>
            <dl className="db-facts">
                <dt>Engine</dt>
                <dd>{engineLabel(profile.engine)}</dd>
                {isServer(profile) ? (
                    <>
                        <dt>Server</dt>
                        <dd className="mono">
                            {profile.host}:{profile.port ?? defaultPort(profile.engine)}
                        </dd>
                        <dt>Database</dt>
                        <dd className="mono">{profile.database || profile.user}</dd>
                        <dt>User</dt>
                        <dd className="mono">{profile.user}</dd>
                        <dt>Password</dt>
                        <dd>{profile.hasPassword ? "Saved in the Keychain" : "None"}</dd>
                    </>
                ) : (
                    <>
                        <dt>File</dt>
                        <dd className="mono">{profile.path}</dd>
                    </>
                )}
                <dt>Agents</dt>
                <dd>{profile.agentWrites ? "May change data" : "Read only"}</dd>
                <dt>Status</dt>
                <dd>{connected ? `Connected to ${connected.version}` : "Not connected"}</dd>
            </dl>
            {error && (
                <div className="db-callout" data-tone="danger" role="alert">
                    {error}
                </div>
            )}
            <div className="db-actions">
                <button type="button" className="db-button" onClick={onEdit}>
                    Edit
                </button>
                <button type="button" className={`db-button${connected ? "" : " primary"}`} disabled={busy} onClick={() => void toggle()}>
                    {busy ? (connected ? "Disconnecting…" : "Connecting…") : connected ? "Disconnect" : "Connect"}
                </button>
            </div>
        </article>
    );
}
