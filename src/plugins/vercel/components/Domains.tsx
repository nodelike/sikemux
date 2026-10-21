import { useState } from "react";
import { confirmDialog, copyWithNotice, notify } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Seg, SkeletonRows } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type DomainRouting, type VercelDomain } from "../api";
import { vercel, vercelDomainR, vercelDomainsR } from "../resources";
import { IconExternal, visit } from "./parts";

const refreshDomains = () => vercel.refresh(vercelDomainsR, vercelDomainR);

type Route = "production" | "branch" | "redirect";

/** What the person picked, as the routing Vercel is sent. */
export function routingOf(route: Route, branch: string, redirect: string): DomainRouting {
    if (route === "branch") return { branch: branch.trim(), redirect: "" };
    if (route === "redirect") return { redirect: redirect.trim(), branch: "" };
    return { branch: "", redirect: "" };
}

/** A project's domains as Vercel's Domains page has them: each one's setup, with adding, routing and removing them. */
export function Domains({ active, team, project }: { active: boolean; team: string; project: string }) {
    const domains = useResourceEnabled(active, vercelDomainsR, team, project);
    const [adding, setAdding] = useState(false);
    if (domains.status === "error" && !domains.data) return <p className="vercel-note">{domains.error}</p>;
    if (!domains.data) return <SkeletonRows rows={3} label="Loading domains" />;
    return (
        <div className="vercel-settings">
            <div className="vercel-form-foot">
                <span className="vercel-muted">Domains point visitors at production, at one branch, or on to another domain.</span>
                {adding ? null : (
                    <button type="button" className="vercel-button primary" onClick={() => setAdding(true)}>
                        Add Domain
                    </button>
                )}
            </div>
            {adding ? <DomainForm team={team} project={project} domain={null} onDone={() => setAdding(false)} /> : null}
            {domains.data.length === 0 ? (
                <EmptyState message="No domains." />
            ) : (
                domains.data.map((domain) => <DomainCard key={domain.name} active={active} team={team} project={project} domain={domain} />)
            )}
        </div>
    );
}

function DomainCard({ active, team, project, domain }: { active: boolean; team: string; project: string; domain: VercelDomain }) {
    const detail = useResourceEnabled(active, vercelDomainR, team, project, domain.name);
    const [editing, setEditing] = useState(false);
    const [checking, setChecking] = useState(false);
    const setUp = detail.data ? detail.data.verified && !detail.data.misconfigured : domain.verified;
    const records = detail.data?.records ?? [];

    const check = async () => {
        setChecking(true);
        try {
            const after = await vercelApi.verifyDomain(team, project, domain.name);
            refreshDomains();
            notify(
                after.records.length === 0 ? "success" : "info",
                after.records.length === 0 ? `${domain.name} is set up` : `${domain.name} still needs its DNS records`,
            );
        } catch (error) {
            notify("error", `Check ${domain.name}: ${errorText(error)}`);
        } finally {
            setChecking(false);
        }
    };

    const remove = async () => {
        const sure = await confirmDialog({
            title: `Remove ${domain.name}?`,
            body: `Visitors to ${domain.name} stop reaching ${project}. Its DNS records stay where they are.`,
            confirmLabel: "Remove",
            destructive: true,
        });
        if (!sure) return;
        try {
            await vercelApi.removeDomain(team, project, domain.name);
            refreshDomains();
            notify("success", `Removed ${domain.name}`);
        } catch (error) {
            notify("error", `Remove ${domain.name}: ${errorText(error)}`);
        }
    };

    return (
        <section className="vercel-box vercel-domain" aria-label={domain.name}>
            <div className="vercel-domain-head">
                <button type="button" className="vercel-link vercel-domain-name" onClick={() => visit(`https://${domain.name}`)}>
                    {domain.name}
                    <IconExternal />
                </button>
                <span className="vercel-row-actions">
                    <button type="button" className="vercel-link" disabled={checking} onClick={() => void check()}>
                        {checking ? "Checking…" : "Refresh"}
                    </button>
                    <button type="button" className="vercel-link" onClick={() => setEditing((was) => !was)}>
                        Edit
                    </button>
                    <button type="button" className="vercel-link danger" onClick={() => void remove()}>
                        Remove
                    </button>
                </span>
            </div>
            <span className="vercel-status">
                <span className={`vercel-dot ${setUp ? "ready" : "error"}`} aria-hidden="true" />
                {setUp ? "Valid Configuration" : detail.data && !detail.data.verified ? "Verification Needed" : "Invalid Configuration"}
            </span>
            <span className="vercel-muted">
                {domain.redirect ? `Redirects to ${domain.redirect}` : domain.gitBranch ? `Assigned to the ${domain.gitBranch} branch` : "Production"}
            </span>
            {records.length > 0 ? (
                <div className="vercel-records">
                    <p className="vercel-muted">Add these records at your domain's DNS provider, then Refresh:</p>
                    <div className="vercel-table" role="table" aria-label={`DNS records for ${domain.name}`}>
                        <div className="vercel-table-row record head" role="row">
                            <span>Type</span>
                            <span>Name</span>
                            <span>Value</span>
                        </div>
                        {records.map((record) => (
                            <div
                                key={`${record.type}:${record.name}:${record.value}`}
                                className="vercel-table-row record"
                                role="row"
                                title={record.why}>
                                <span className="vercel-mono">{record.type}</span>
                                <span className="vercel-mono">{record.name}</span>
                                <button
                                    type="button"
                                    className="vercel-link vercel-mono vercel-record-value"
                                    onClick={() => copyWithNotice(record.value, `${record.type} value`)}>
                                    {record.value}
                                </button>
                            </div>
                        ))}
                    </div>
                </div>
            ) : null}
            {editing ? <DomainForm team={team} project={project} domain={domain} onDone={() => setEditing(false)} /> : null}
        </section>
    );
}

const ROUTES = [
    { value: "production" as const, label: "Production" },
    { value: "branch" as const, label: "A branch" },
    { value: "redirect" as const, label: "Redirect" },
];

/** Adds a domain, or changes where one points. */
function DomainForm({ team, project, domain, onDone }: { team: string; project: string; domain: VercelDomain | null; onDone: () => void }) {
    const [name, setName] = useState(domain?.name ?? "");
    const [route, setRoute] = useState<Route>(domain?.redirect ? "redirect" : domain?.gitBranch ? "branch" : "production");
    const [branch, setBranch] = useState(domain?.gitBranch ?? "");
    const [redirect, setRedirect] = useState(domain?.redirect ?? "");
    const [saving, setSaving] = useState(false);
    const missing = (!domain && !name.trim()) || (route === "branch" && !branch.trim()) || (route === "redirect" && !redirect.trim());

    const save = async () => {
        setSaving(true);
        const routing = routingOf(route, branch, redirect);
        try {
            if (domain) await vercelApi.updateDomain(team, project, domain.name, routing);
            else await vercelApi.addDomain(team, project, name.trim(), route === "production" ? {} : routing);
            refreshDomains();
            notify("success", domain ? `Saved ${domain.name}` : `Added ${name.trim()}`);
            onDone();
        } catch (error) {
            notify("error", `${domain ? "Save" : "Add"} ${domain?.name ?? name.trim()}: ${errorText(error)}`);
        } finally {
            setSaving(false);
        }
    };

    return (
        <form
            className="vercel-box vercel-form-box"
            aria-label={domain ? `Edit ${domain.name}` : "Add Domain"}
            onSubmit={(event) => {
                event.preventDefault();
                if (!missing && !saving) void save();
            }}>
            {domain ? null : (
                <label className="vercel-form-stack">
                    <span>Domain</span>
                    <input
                        className="vercel-find vercel-mono"
                        value={name}
                        onChange={(event) => setName(event.target.value)}
                        placeholder="shop.example.com"
                        spellCheck={false}
                        autoFocus
                    />
                </label>
            )}
            <Seg label="Points at" options={ROUTES} value={route} onChange={setRoute} />
            {route === "branch" ? (
                <label className="vercel-form-stack">
                    <span>Git branch</span>
                    <input
                        className="vercel-find vercel-mono"
                        value={branch}
                        onChange={(event) => setBranch(event.target.value)}
                        placeholder="staging"
                        spellCheck={false}
                    />
                </label>
            ) : null}
            {route === "redirect" ? (
                <label className="vercel-form-stack">
                    <span>Redirect to</span>
                    <input
                        className="vercel-find vercel-mono"
                        value={redirect}
                        onChange={(event) => setRedirect(event.target.value)}
                        placeholder="example.com"
                        spellCheck={false}
                    />
                </label>
            ) : null}
            <div className="vercel-form-foot">
                <button type="button" className="vercel-button" onClick={onDone}>
                    Cancel
                </button>
                <button type="submit" className="vercel-button primary" disabled={missing || saving}>
                    {saving ? "Saving…" : domain ? "Save" : "Add"}
                </button>
            </div>
        </form>
    );
}
