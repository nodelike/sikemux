import { Fragment, useState } from "react";
import { errorText } from "../../../plugin-api/format";
import { databaseApi } from "../api";
import type { DatabaseConnection } from "../types.generated";

/** A saved connection that is not open: where it points, and a button to connect. */
export function ProfileDetail({ profile, onEdit }: { profile: DatabaseConnection; onEdit: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const connected = profile.connected !== null;

    const toggle = async () => {
        setBusy(true);
        setError(null);
        try {
            if (connected) await databaseApi.disconnect(profile.id);
            else await databaseApi.connect(profile.id);
        } catch (failure) {
            setError(errorText(failure));
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
                {profile.facts.map((fact) => (
                    <Fragment key={fact.label}>
                        <dt>{fact.label}</dt>
                        <dd className={fact.mono ? "mono" : undefined}>{fact.value}</dd>
                    </Fragment>
                ))}
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
