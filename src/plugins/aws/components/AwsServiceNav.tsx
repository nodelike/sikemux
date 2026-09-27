import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconChevron } from "../../../plugin-api/ui";
import { awsIdentityR, awsProfilesR } from "../resources";
import { AWS_SERVICES, awsSettings, setAwsProfile, setAwsService, useAws, type AwsService } from "../state";
import { ServiceGlyph } from "./icons";
import { State } from "./parts";

const META: Record<AwsService, { label: string; hint: string }> = {
    ecs: { label: "ECS", hint: "Clusters, services, tasks and logs" },
    ec2: { label: "EC2", hint: "Instances" },
    lambda: { label: "Lambda", hint: "Functions and their logs" },
    sqs: { label: "SQS", hint: "Queues" },
    billing: { label: "Billing", hint: "Costs by month" },
    s3: { label: "S3", hint: "Buckets" },
};

export function AwsServiceNav({ profile, signedIn }: { profile: string; signedIn: boolean }) {
    const active = awsSettings.useSelect((s) => s.service);
    const counts = useAws((s) => s.counts[profile]);
    return (
        <nav className="aws-nav" aria-label="AWS services">
            <div className="aws-nav-label">Services</div>
            {AWS_SERVICES.map((s, i) => {
                const sel = signedIn && active === s;
                return (
                    <button
                        key={s}
                        className={`aws-nav-item${sel ? " active" : ""}`}
                        aria-current={sel ? "page" : undefined}
                        onClick={() => setAwsService(s)}
                        disabled={!signedIn}
                        title={`${META[s].hint} · ${i + 1}`}>
                        <span className="aws-nav-icon">
                            <ServiceGlyph service={s} />
                        </span>
                        <span className="aws-nav-name">{META[s].label}</span>
                        {signedIn && counts?.[s] && <span className="aws-nav-count">{counts[s]}</span>}
                    </button>
                );
            })}
            <Account profile={profile} />
        </nav>
    );
}

const formatAccount = (id: string) => id.replace(/^(\d{4})(\d{4})(\d{4})$/, "$1-$2-$3");

/** SSO profiles are often named `<role>-<account id>`; the role reads as the name and the id as its detail. */
function splitProfile(name: string): { title: string; account: string | null } {
    const m = /^(.+?)[-_](\d{12})$/.exec(name);
    return m ? { title: m[1], account: m[2] } : { title: name, account: null };
}

function Account({ profile }: { profile: string }) {
    const profiles = useResource(awsProfilesR).data ?? [];
    const identity = useResourceEnabled(true, awsIdentityR, profile, false);
    const [open, setOpen] = useState(false);
    const root = useRef<HTMLDivElement>(null);
    const current = profiles.find((p) => p.name === profile);
    const { title } = splitProfile(profile);
    const account = identity.data?.account ?? current?.sso_account_id ?? splitProfile(profile).account;
    const status = identity.data?.status;

    useEffect(() => {
        if (!open) return;
        const away = (event: MouseEvent) => {
            if (!root.current?.contains(event.target as Node)) setOpen(false);
        };
        document.addEventListener("mousedown", away);
        return () => document.removeEventListener("mousedown", away);
    }, [open]);

    const onMenuKey = (event: KeyboardEvent<HTMLDivElement>) => {
        const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("[role=menuitemradio]")];
        const at = items.indexOf(document.activeElement as HTMLButtonElement);
        if (event.key === "Escape") {
            event.stopPropagation();
            setOpen(false);
            root.current?.querySelector<HTMLButtonElement>(".aws-account")?.focus();
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = (at + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
            items[next]?.focus();
        }
    };

    return (
        <div className="aws-account-wrap" ref={root}>
            {open && (
                <div className="aws-profile-menu" role="menu" aria-label="AWS profiles" onKeyDown={onMenuKey}>
                    <div className="aws-profile-menu-label">Profiles</div>
                    {profiles.map((p) => {
                        const split = splitProfile(p.name);
                        const id = p.sso_account_id ?? split.account;
                        const on = p.name === profile;
                        return (
                            <button
                                key={p.name}
                                role="menuitemradio"
                                aria-checked={on}
                                className={`aws-profile-item${on ? " on" : ""}`}
                                title={p.name}
                                autoFocus={on}
                                onClick={() => {
                                    setOpen(false);
                                    if (!on) setAwsProfile(p.name);
                                }}>
                                <span className="aws-profile-item-name">{split.title}</span>
                                <span className="aws-profile-item-meta">
                                    {[id && formatAccount(id), p.kind === "sso" ? "SSO" : p.kind, p.region].filter(Boolean).join(" · ")}
                                </span>
                            </button>
                        );
                    })}
                </div>
            )}
            <button
                className={`aws-account${open ? " open" : ""}`}
                onClick={() => setOpen((v) => !v)}
                aria-haspopup="menu"
                aria-expanded={open}
                title={`${profile} · switch profile`}>
                <span className="aws-account-top">
                    <span className="aws-account-who">
                        <span className="aws-account-name">{title}</span>
                        <span className="aws-account-id">{account ? formatAccount(account) : "—"}</span>
                    </span>
                    <IconChevron size={11} className="aws-account-chev" />
                </span>
                <span className="aws-account-meta">
                    <span>{current?.region ?? "no region"}</span>
                    {status === "authed" ? (
                        <State health="ok" label="Signed in" />
                    ) : status ? (
                        <State health="warn" label={status === "expired" ? "Expired" : "Signed out"} />
                    ) : (
                        <State health="off" label="Checking" />
                    )}
                </span>
            </button>
        </div>
    );
}
