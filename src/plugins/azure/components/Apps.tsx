import { useState } from "react";
import { notify, openUrl, swallow } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { azure } from "../resources";
import { azureApi, type App, type AppAction, type Scope } from "../api";
import { appTone, region } from "../format";
import { Dot } from "./parts";

export function Apps({ apps, scope, showGroup, onOpen }: { apps: App[]; scope: Scope; showGroup: boolean; onOpen: (app: App) => void }) {
    if (apps.length === 0) return <p className="az-note">No App Service or Functions apps here.</p>;
    return (
        <section className="az-section" aria-label="Apps">
            <table className="az-table" aria-label="Apps">
                <thead>
                    <tr>
                        <th aria-label="State" />
                        <th>Name</th>
                        <th>State</th>
                        <th>Kind</th>
                        <th>Runtime</th>
                        {showGroup ? <th>Resource group</th> : null}
                        <th>Region</th>
                        <th aria-label="Actions" />
                    </tr>
                </thead>
                <tbody>
                    {apps.map((app) => (
                        <tr
                            key={app.id}
                            className="az-row"
                            tabIndex={0}
                            aria-label={app.name}
                            onClick={() => onOpen(app)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") onOpen(app);
                            }}>
                            <td className="az-cell-dot">
                                <Dot tone={appTone(app.state)} label={app.state} />
                            </td>
                            <td>
                                <span className="az-name">{app.name}</span>
                                {app.host ? (
                                    <button
                                        type="button"
                                        className="az-note-inline az-link"
                                        title={`Open https://${app.host}`}
                                        onClick={(event) => {
                                            event.stopPropagation();
                                            void openUrl(`https://${app.host}`).catch(swallow("open the app"));
                                        }}>
                                        {app.host}
                                    </button>
                                ) : null}
                            </td>
                            <td className={`az-state ${appTone(app.state)}`}>{app.state}</td>
                            <td className="az-muted">{app.functions ? "Functions" : "Web app"}</td>
                            <td className="az-mono az-muted">{app.runtime ?? "—"}</td>
                            {showGroup ? <td className="az-muted">{app.group}</td> : null}
                            <td className="az-muted">{region(app.location)}</td>
                            <td className="az-cell-actions">
                                <AppActions app={app} scope={scope} />
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </section>
    );
}

export function AppActions({ app, scope }: { app: App; scope: Scope }) {
    const [busy, setBusy] = useState<AppAction | null>(null);
    const run = async (action: AppAction) => {
        setBusy(action);
        try {
            const done = await azureApi.appAct({ ...scope, group: app.group }, app.name, action);
            notify("success", done.message);
            azure.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };
    const actions: AppAction[] = app.state === "Running" ? ["restart", "stop"] : ["start"];
    return (
        <span className="az-row-actions">
            {actions.map((action) => (
                <button
                    key={action}
                    type="button"
                    className="az-button small"
                    disabled={busy !== null}
                    aria-label={`${action[0].toUpperCase()}${action.slice(1)} ${app.name}`}
                    onClick={(event) => {
                        event.stopPropagation();
                        void run(action);
                    }}
                    onKeyDown={(event) => event.stopPropagation()}>
                    {busy === action ? "…" : `${action[0].toUpperCase()}${action.slice(1)}`}
                </button>
            ))}
        </span>
    );
}
