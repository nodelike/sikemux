import { useResourceEnabled } from "../../../plugin-api/resources";
import { iamMembersR, serviceAccountsR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { Button, consoleUrl, keyOf, Loaded, PageHeader, Status, Table, Tabs, visit } from "./parts";

const LISTS = [
    { value: "members", label: "Permissions" },
    { value: "accounts", label: "Service accounts" },
] as const;

const KINDS: Record<string, string> = { user: "User", group: "Group", serviceAccount: "Service account", domain: "Domain", deleted: "Deleted" };

export function IamPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = view.list === "accounts" ? "accounts" : "members";
    const members = useResourceEnabled(active && list === "members", iamMembersR, ...keyOf(target));
    const accounts = useResourceEnabled(active && list === "accounts", serviceAccountsR, ...keyOf(target));
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="IAM">
                <Button onClick={() => visit(consoleUrl(list === "accounts" ? "iam-admin/serviceaccounts" : "iam-admin/iam", target.project))}>
                    Grant access in Cloud console
                </Button>
            </PageHeader>
            <Tabs label="IAM" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value })} />
            <Filter paneId={paneId} placeholder={list === "accounts" ? "Filter service accounts" : "Filter by member or role"} />
            {list === "members" ? (
                <Loaded handle={members} label="permissions" empty={(rows) => (rows.length === 0 ? "No one holds a role on this project." : null)}>
                    {(rows) => (
                        <Table
                            label="Permissions"
                            rows={rows.filter(
                                (member) =>
                                    !words ||
                                    member.member.toLowerCase().includes(words) ||
                                    member.roles.some((role) => role.toLowerCase().includes(words)),
                            )}
                            rowKey={(member) => `${member.kind}:${member.member}`}
                            columns={[
                                { label: "Principal", width: "minmax(200px, 1.6fr)", cell: (member) => member.member },
                                { label: "Type", width: "120px", cell: (member) => KINDS[member.kind] ?? member.kind },
                                {
                                    label: "Roles",
                                    width: "minmax(200px, 2fr)",
                                    cell: (member) => (
                                        <span className="gcp-roles">
                                            {member.roles.map((role) => (
                                                <span key={role} className="gcp-chip">
                                                    {role}
                                                </span>
                                            ))}
                                        </span>
                                    ),
                                },
                            ]}
                        />
                    )}
                </Loaded>
            ) : (
                <Loaded
                    handle={accounts}
                    label="service accounts"
                    empty={(rows) => (rows.length === 0 ? "No service accounts in this project." : null)}>
                    {(rows) => (
                        <Table
                            label="Service accounts"
                            rows={rows.filter(
                                (account) => !words || account.email.toLowerCase().includes(words) || account.name?.toLowerCase().includes(words),
                            )}
                            rowKey={(account) => account.email}
                            columns={[
                                { label: "", width: "24px", cell: (account) => <Status state={account.disabled ? "DISABLED" : "ENABLED"} /> },
                                { label: "Email", width: "minmax(220px, 2fr)", cell: (account) => account.email },
                                { label: "Name", cell: (account) => account.name ?? "—" },
                                {
                                    label: "Description",
                                    width: "minmax(160px, 1.4fr)",
                                    cell: (account) => <span className="gcp-muted">{account.description ?? ""}</span>,
                                },
                            ]}
                        />
                    )}
                </Loaded>
            )}
            <p className="gcp-note">Read only. Grant or remove access in the Cloud console.</p>
        </div>
    );
}
