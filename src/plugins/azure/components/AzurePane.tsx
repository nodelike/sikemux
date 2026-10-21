import { useRef, useState, type ReactNode } from "react";
import { notify } from "../../../plugin-api/host";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { Dropdown, ResizeHandle, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { azureApi, type AzureStatus, type Scope } from "../api";
import { azure, azureActivityR, azureAppsR, azureGroupsR, azureResourcesR, azureStorageR, azureVmsR } from "../resources";
import { ALL_GROUPS, updateAzureView, useAzureView, type AzureSection } from "../state";
import { Activity } from "./Activity";
import { AppView } from "./AppView";
import { Apps } from "./Apps";
import { AzureMark } from "./AzureMark";
import { AzureSignIn } from "./AzureSignIn";
import { MachineView } from "./MachineView";
import { Machines } from "./Machines";
import { Resources } from "./Resources";
import { Storage } from "./Storage";
import "../azure.css";

const SIDE_MIN = 180;
const SIDE_MAX = 360;

const SECTIONS: { value: AzureSection; label: string }[] = [
    { value: "resources", label: "All resources" },
    { value: "compute", label: "Virtual machines" },
    { value: "web", label: "Apps" },
    { value: "storage", label: "Storage" },
    { value: "activity", label: "Activity" },
];

export function AzurePane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, azure.statusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Reading your Azure sign-in" />;
    if (!status.data?.ready) return <AzureSignIn status={status.data} onSignedIn={() => azure.refresh()} />;
    return <Portal paneId={paneId} active={active} status={status.data} />;
}

function Portal({ paneId, active, status }: { paneId: string; active: boolean; status: AzureStatus }) {
    const view = useAzureView(paneId);
    const sideRef = useRef<HTMLDivElement>(null);
    const [words, setWords] = useState(view.find);
    const subscription = view.subscription ?? status.current?.id ?? status.subscriptions[0]?.id ?? "";
    const chosen = status.subscriptions.find((each) => each.id === subscription);
    const all = view.group === ALL_GROUPS;
    const scope: Scope = { subscription, group: all ? undefined : view.group };
    const listing = active && view.open === null;
    const groups = useResourceEnabled(active, azureGroupsR, subscription);
    const inventory = useResourceEnabled(listing && view.section === "resources", azureResourcesR, scope, view.find);
    const machines = useResourceEnabled(listing && view.section === "compute", azureVmsR, scope);
    const apps = useResourceEnabled(listing && view.section === "web", azureAppsR, scope);
    const storage = useResourceEnabled(listing && view.section === "storage", azureStorageR, scope);
    const activity = useResourceEnabled(listing && view.section === "activity", azureActivityR, scope, view.hours, view.failedOnly);

    const chooseSubscription = async (next: string) => {
        updateAzureView(paneId, { subscription: next, group: ALL_GROUPS, open: null });
        try {
            await azureApi.useSubscription(next);
            azure.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        }
    };

    return (
        <div className="az-pane">
            <aside className="az-side" ref={sideRef} style={view.sideWidth ? { width: view.sideWidth } : undefined}>
                <Dropdown
                    className="az-pick"
                    label="Subscription"
                    value={subscription}
                    icon={
                        <span className="az-pick-mark">
                            <AzureMark size={12} className="icon-azure" />
                        </span>
                    }
                    options={status.subscriptions.map((each) => ({
                        value: each.id,
                        label: each.name,
                        detail: each.state === "Enabled" ? undefined : each.state,
                    }))}
                    onChange={(next) => void chooseSubscription(next)}
                />
                <Dropdown
                    className="az-pick"
                    label="Resource group"
                    value={view.group}
                    options={[
                        { value: ALL_GROUPS, label: "All resource groups" },
                        ...(groups.data ?? []).map((group) => ({ value: group.name, label: group.name, detail: group.location })),
                    ]}
                    onChange={(next) => updateAzureView(paneId, { group: next, open: null })}
                />
                <input
                    className="az-find"
                    value={words}
                    onChange={(event) => setWords(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") updateAzureView(paneId, { find: words.trim(), section: "resources", open: null });
                        if (event.key === "Escape") {
                            setWords("");
                            updateAzureView(paneId, { find: "" });
                        }
                    }}
                    placeholder="Find a resource…"
                    aria-label="Find a resource"
                    title="Words a resource's name must contain. Enter searches."
                    spellCheck={false}
                />
                <nav className="az-nav" aria-label="Azure">
                    {SECTIONS.map((section) => (
                        <button
                            key={section.value}
                            type="button"
                            className={`az-nav-item${view.section === section.value ? " active" : ""}`}
                            aria-current={view.section === section.value ? "page" : undefined}
                            onClick={() => updateAzureView(paneId, { section: section.value, open: null })}>
                            {section.label}
                        </button>
                    ))}
                </nav>
                <div className="az-side-foot">
                    <span className="az-side-user">{status.user || chosen?.user || "Signed in"}</span>
                    {status.cliVersion ? <span className="az-muted az-mono">az {status.cliVersion}</span> : null}
                </div>
            </aside>
            <ResizeHandle
                targetRef={sideRef}
                axis="x"
                grows={1}
                min={SIDE_MIN}
                max={() => SIDE_MAX}
                size={view.sideWidth}
                label="Resize the Azure sidebar"
                className="az-resize"
                onResize={(size) => {
                    if (size === null && sideRef.current) sideRef.current.style.width = "";
                    updateAzureView(paneId, { sideWidth: size });
                }}
            />
            <main className="az-main">
                <header className="az-topbar">
                    <span className="az-topbar-title">{SECTIONS.find((each) => each.value === view.section)?.label}</span>
                    <span className="az-muted">
                        {all ? "All resource groups" : view.group}
                        {view.find ? ` · “${view.find}”` : ""}
                    </span>
                    <button type="button" className="az-button" onClick={() => azure.refresh()}>
                        Refresh
                    </button>
                </header>
                <div className="az-page">
                    {view.open && view.section === "compute" ? (
                        <MachineView
                            key={`${view.open.group}/${view.open.name}`}
                            scope={{ subscription }}
                            group={view.open.group}
                            name={view.open.name}
                            active={active}
                            onBack={() => updateAzureView(paneId, { open: null })}
                        />
                    ) : view.open && view.section === "web" ? (
                        <AppView
                            key={`${view.open.group}/${view.open.name}`}
                            scope={{ subscription }}
                            group={view.open.group}
                            name={view.open.name}
                            active={active}
                            onBack={() => updateAzureView(paneId, { open: null })}
                        />
                    ) : view.section === "compute" ? (
                        <Loaded handle={machines} label="virtual machines">
                            {(data) => (
                                <Machines
                                    machines={data}
                                    scope={{ subscription }}
                                    showGroup={all}
                                    onOpen={(machine) => updateAzureView(paneId, { open: { group: machine.group, name: machine.name } })}
                                />
                            )}
                        </Loaded>
                    ) : view.section === "web" ? (
                        <Loaded handle={apps} label="apps">
                            {(data) => (
                                <Apps
                                    apps={data}
                                    scope={{ subscription }}
                                    showGroup={all}
                                    onOpen={(app) => updateAzureView(paneId, { open: { group: app.group, name: app.name } })}
                                />
                            )}
                        </Loaded>
                    ) : view.section === "storage" ? (
                        <Loaded handle={storage} label="storage">
                            {(data) => <Storage accounts={data} scope={scope} showGroup={all} active={active} />}
                        </Loaded>
                    ) : view.section === "activity" ? (
                        <Loaded handle={activity} label="activity">
                            {(data) => (
                                <Activity
                                    events={data}
                                    hours={view.hours}
                                    failedOnly={view.failedOnly}
                                    showGroup={all}
                                    onWindow={(change) => updateAzureView(paneId, change)}
                                />
                            )}
                        </Loaded>
                    ) : (
                        <Loaded handle={inventory} label="resources">
                            {(data) => <Resources inventory={data} showGroup={all} onOpenGroup={(group) => updateAzureView(paneId, { group })} />}
                        </Loaded>
                    )}
                </div>
            </main>
        </div>
    );
}

function Loaded<T>({ handle, label, children }: { handle: ResourceHandle<T>; label: string; children: (data: T) => ReactNode }) {
    if (handle.status === "error" && handle.data === undefined) return <p className="az-note az-error">{handle.error}</p>;
    if (handle.data === undefined) return <SkeletonRows rows={6} label={`Loading ${label}`} />;
    return (
        <>
            <StaleNotice handle={handle} />
            {children(handle.data)}
        </>
    );
}
