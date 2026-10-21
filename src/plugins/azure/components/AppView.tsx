import { useEffect, useMemo, useState } from "react";
import { copyWithNotice, openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Seg, SkeletonRows, StaleNotice, VirtualLogList } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { azureApi, type AppDetail, type Scope } from "../api";
import { appTone, region } from "../format";
import { azureAppR } from "../resources";
import { AppActions } from "./Apps";
import { Dot, Fact, Tags } from "./parts";

/** The most lines kept while following, so a chatty app can't grow the pane without end. */
const KEEP_LINES = 5_000;

type AppTab = "overview" | "logs" | "settings";

export function AppView({ scope, group, name, active, onBack }: { scope: Scope; group: string; name: string; active: boolean; onBack: () => void }) {
    const [tab, setTab] = useState<AppTab>("overview");
    const app = useResourceEnabled(active, azureAppR, { ...scope, group }, name);
    const found = app.data;

    return (
        <section className="az-detail" aria-label={`App ${name}`}>
            <header className="az-detail-head">
                <button type="button" className="az-back" onClick={onBack} aria-label="Back to apps">
                    ‹ Apps
                </button>
                <div className="az-detail-title">
                    {found ? <Dot tone={appTone(found.state)} label={found.state} /> : null}
                    <h2 className="az-name">{name}</h2>
                    {found ? <span className={`az-state ${appTone(found.state)}`}>{found.state}</span> : null}
                </div>
                <div className="az-detail-actions">
                    {found?.host ? (
                        <button
                            type="button"
                            className="az-button"
                            onClick={() => void openUrl(`https://${found.host}`).catch(swallow("open the app"))}>
                            Open
                        </button>
                    ) : null}
                    {found ? <AppActions app={found} scope={scope} /> : null}
                </div>
            </header>
            {app.status === "error" && !found ? (
                <p className="az-note az-error">{app.error}</p>
            ) : !found ? (
                <SkeletonRows rows={6} label="Reading the app" />
            ) : (
                <>
                    <StaleNotice handle={app} />
                    <Seg
                        label="App view"
                        value={tab}
                        onChange={setTab}
                        options={[
                            { value: "overview", label: "Overview" },
                            { value: "logs", label: "Logs" },
                            { value: "settings", label: "Settings", count: found.settings.length },
                        ]}
                    />
                    {tab === "logs" ? (
                        <AppLogs scope={{ ...scope, group }} name={name} />
                    ) : tab === "settings" ? (
                        <Settings app={found} />
                    ) : (
                        <Overview app={found} />
                    )}
                </>
            )}
        </section>
    );
}

function Overview({ app }: { app: AppDetail }) {
    return (
        <>
            <dl className="az-facts">
                <Fact term="Address" value={app.host} mono />
                <Fact term="Kind" value={app.functions ? "Functions app" : "Web app"} />
                <Fact term="Runtime" value={app.runtime} mono />
                <Fact term="Plan" value={app.plan} />
                <Fact term="Region" value={region(app.location)} />
                <Fact term="Resource group" value={app.group} />
                <Fact term="HTTPS only" value={app.httpsOnly ? "Yes" : "No"} />
                <Fact term="Always on" value={app.alwaysOn ? "Yes" : "No"} />
                <Fact term="Inbound IP" value={app.inboundIp} mono />
            </dl>
            {app.outboundIps.length > 0 ? (
                <>
                    <h3 className="az-heading">Calls out from</h3>
                    <p className="az-mono az-muted">{app.outboundIps.join(", ")}</p>
                </>
            ) : null}
            {app.slots.length > 0 ? (
                <>
                    <h3 className="az-heading">Deployment slots</h3>
                    <ul className="az-pairs" aria-label="Slots">
                        {app.slots.map((slot) => (
                            <li key={slot.name}>
                                <span className="az-name">{slot.name}</span>
                                <span className="az-muted">{slot.state}</span>
                                {slot.host ? (
                                    <button
                                        type="button"
                                        className="az-link az-mono"
                                        onClick={() => void openUrl(`https://${slot.host}`).catch(swallow("open the slot"))}>
                                        {slot.host}
                                    </button>
                                ) : null}
                            </li>
                        ))}
                    </ul>
                </>
            ) : null}
            <Tags tags={app.tags} />
        </>
    );
}

function Settings({ app }: { app: AppDetail }) {
    if (app.settings.length === 0) return <p className="az-note">This app has no settings of its own.</p>;
    return (
        <section className="az-section" aria-label="Settings">
            <p className="az-note">Only the names are read. Values never leave Azure.</p>
            <ul className="az-pairs" aria-label="App settings">
                {app.settings.map((setting) => (
                    <li key={setting.name}>
                        <button
                            type="button"
                            className="az-mono az-link"
                            title="Copy the name"
                            onClick={() => copyWithNotice(setting.name, "The setting name")}>
                            {setting.name}
                        </button>
                        {setting.fromKeyVault ? <span className="az-chip">from Key Vault</span> : null}
                    </li>
                ))}
            </ul>
        </section>
    );
}

export function AppLogs({ scope, name }: { scope: Scope; name: string }) {
    const [lines, setLines] = useState<string[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [find, setFind] = useState("");
    const { subscription, group } = scope;

    useEffect(() => {
        setLines(null);
        setError(null);
        const stream = azureApi.tailAppLogs({ subscription, group }, name, {
            onItem: (item) => setLines((held) => [...(held ?? []), item.line].slice(-KEEP_LINES)),
            onError: (failure) => setError(errorText(failure)),
        });
        return () => stream.stop();
    }, [subscription, group, name]);

    const shown = useMemo(() => {
        const words = find.trim().toLowerCase();
        return words ? (lines ?? []).filter((line) => line.toLowerCase().includes(words)) : (lines ?? []);
    }, [lines, find]);

    return (
        <section className="az-logs" aria-label="Logs">
            <div className="az-toolbar">
                <input
                    className="az-find az-find-inline"
                    value={find}
                    onChange={(event) => setFind(event.target.value)}
                    placeholder="Filter lines…"
                    aria-label="Filter lines"
                    spellCheck={false}
                />
                <span className="az-muted az-logs-count">
                    {error ? null : <span className="az-live">Live</span>}
                    {lines ? `${shown.length.toLocaleString()} lines` : ""}
                </span>
            </div>
            {error ? (
                <p className="az-note az-error">{error}</p>
            ) : lines === null ? (
                <p className="az-note">Waiting for the app to log something. Turn on application logging in Azure if nothing comes.</p>
            ) : (
                <VirtualLogList
                    items={shown}
                    className="az-log-list"
                    rowClassName="az-log-line"
                    follow
                    allowFollow={(element) => element.scrollHeight - element.scrollTop - element.clientHeight < 80}
                    empty={<p className="az-note">{find ? "No lines match." : "Nothing logged yet."}</p>}
                    renderRow={(line) => <span className="az-log-text">{line}</span>}
                />
            )}
        </section>
    );
}
