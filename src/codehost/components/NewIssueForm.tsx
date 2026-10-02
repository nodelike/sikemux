import { useState } from "react";
import { notify, reportError } from "../../plugin-api/host";
import { invalidate } from "../../plugin-api/resources";
import { IconClose } from "../../plugin-api/ui";
import { hostApi, type RepoRef } from "../api";

interface Props {
    repo: RepoRef;
    onCreated: (number: number) => void;
    onCancel: () => void;
}

export function NewIssueForm({ repo, onCreated, onCancel }: Props) {
    const [title, setTitle] = useState("");
    const [body, setBody] = useState("");
    const [busy, setBusy] = useState(false);

    const create = async () => {
        setBusy(true);
        try {
            const made = await hostApi(repo.provider).createIssue(repo, title.trim(), body);
            notify("success", `Opened #${made.number}`);
            invalidate((kind) => kind === "host.issues");
            onCreated(made.number);
        } catch (error) {
            reportError("Could not open the issue")(error);
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="gha-detail gha-form">
            <button type="button" className="gha-back" onClick={onCancel}>
                <IconClose size={11} /> Back to issues
            </button>
            <h2 className="gha-title">New issue</h2>
            <input className="gha-input" value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Title" aria-label="Title" />
            <textarea
                className="gha-input gha-comment-box"
                value={body}
                onChange={(event) => setBody(event.target.value)}
                placeholder="What happened, and what you expected"
                rows={8}
                aria-label="Description"
            />
            <div className="gha-detail-actions">
                <button type="button" className="gha-btn primary" disabled={!title.trim() || busy} onClick={() => void create()}>
                    {busy ? "Opening…" : "Open issue"}
                </button>
                <button type="button" className="gha-link" onClick={onCancel}>
                    Cancel
                </button>
            </div>
        </div>
    );
}
