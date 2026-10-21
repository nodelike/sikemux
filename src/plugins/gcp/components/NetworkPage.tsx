import { useResourceEnabled } from "../../../plugin-api/resources";
import { gcpApi, type FirewallRule } from "../api";
import { networkR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { Button, consoleUrl, keyOf, Loaded, PageHeader, Status, Table, Tabs, useAction, visit } from "./parts";

const LISTS = [
    { value: "firewall", label: "Firewall" },
    { value: "networks", label: "VPC networks" },
    { value: "subnets", label: "Subnets" },
] as const;

/** Whether a rule lets a port through: `5432` matches tcp:5432, a range holding it, or a rule with no ports. */
export function opensPort(rule: FirewallRule, words: string): boolean {
    const port = Number(words);
    if (!Number.isInteger(port) || port <= 0) return false;
    return rule.protocols.some((protocol) => {
        const [name, ports] = protocol.split(":");
        if (!ports) return name === "all" || name === "tcp" || name === "udp";
        return ports.split(",").some((range) => {
            const [low, high] = range.split("-").map(Number);
            return port >= low && port <= (high ?? low);
        });
    });
}

export function NetworkPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = LISTS.find((each) => each.value === view.list)?.value ?? "firewall";
    const network = useResourceEnabled(active, networkR, ...keyOf(target));
    const action = useAction();
    const words = view.filter.trim().toLowerCase();
    const toggle = (rule: FirewallRule) =>
        void action.run(
            {
                title: `${rule.disabled ? "Turn on" : "Turn off"} ${rule.name}?`,
                body: rule.disabled
                    ? `${rule.name} ${rule.action}s traffic again.`
                    : `${rule.name} stops applying; traffic it ${rule.action}ed falls to the next rule by priority.`,
                confirmLabel: rule.disabled ? "Turn on" : "Turn off",
                destructive: !rule.disabled,
            },
            `${rule.name} is ${rule.disabled ? "on" : "off"}`,
            () => gcpApi.firewallToggle(target, rule.name, !rule.disabled),
        );
    return (
        <div className="gcp-page">
            <PageHeader title="VPC network">
                <Button
                    onClick={() =>
                        visit(
                            consoleUrl(
                                list === "firewall" ? "net-security/firewall-manager/firewall-policies/list" : "networking/networks/list",
                                target.project,
                            ),
                        )
                    }>
                    Open in Cloud console
                </Button>
            </PageHeader>
            <Tabs label="VPC network" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value, filter: "" })} />
            <Filter paneId={paneId} placeholder={list === "firewall" ? "Filter rules, or a port such as 5432" : "Filter"} />
            <Loaded handle={network} label="networks" empty={(data) => (data.networks.length === 0 ? "No VPC networks in this project." : null)}>
                {(data) =>
                    list === "firewall" ? (
                        <Table
                            label="Firewall rules"
                            rows={data.firewalls.filter(
                                (rule) =>
                                    !words ||
                                    rule.name.includes(words) ||
                                    rule.network.includes(words) ||
                                    rule.targets.some((each) => each.includes(words)) ||
                                    opensPort(rule, words),
                            )}
                            rowKey={(rule) => rule.name}
                            columns={[
                                { label: "", width: "24px", cell: (rule) => <Status state={rule.disabled ? "DISABLED" : "ENABLED"} /> },
                                {
                                    label: "Name",
                                    width: "minmax(160px, 1.2fr)",
                                    cell: (rule) => <span title={rule.description ?? undefined}>{rule.name}</span>,
                                },
                                { label: "Direction", width: "80px", cell: (rule) => (rule.direction === "INGRESS" ? "Ingress" : "Egress") },
                                {
                                    label: "Action",
                                    width: "60px",
                                    cell: (rule) => (
                                        <span className={rule.action === "deny" ? "gcp-error-text" : undefined}>
                                            {rule.action === "deny" ? "Deny" : "Allow"}
                                        </span>
                                    ),
                                },
                                { label: "Protocols and ports", width: "minmax(130px, 1fr)", cell: (rule) => rule.protocols.join(", ") || "all" },
                                { label: "Sources", width: "minmax(130px, 1fr)", cell: (rule) => rule.sources.join(", ") || "—" },
                                { label: "Targets", width: "minmax(110px, 1fr)", cell: (rule) => rule.targets.join(", ") || "All instances" },
                                { label: "Priority", width: "70px", cell: (rule) => rule.priority },
                                {
                                    label: "",
                                    width: "100px",
                                    align: "end",
                                    cell: (rule) => (
                                        <Button danger={!rule.disabled} disabled={action.busy} onClick={() => toggle(rule)}>
                                            {rule.disabled ? "Turn on" : "Turn off"}
                                        </Button>
                                    ),
                                },
                            ]}
                        />
                    ) : list === "networks" ? (
                        <Table
                            label="VPC networks"
                            rows={data.networks.filter((each) => !words || each.name.includes(words))}
                            rowKey={(each) => each.name}
                            columns={[
                                { label: "Name", width: "minmax(160px, 1.4fr)", cell: (each) => each.name },
                                { label: "Subnet mode", cell: (each) => (each.mode === "auto" ? "Auto" : "Custom") },
                                { label: "Subnets", cell: (each) => each.subnets },
                                { label: "Routing", cell: (each) => each.routing?.toLowerCase() ?? "—" },
                                { label: "MTU", cell: (each) => each.mtu ?? 1460 },
                            ]}
                        />
                    ) : (
                        <Table
                            label="Subnets"
                            rows={data.subnets.filter(
                                (each) => !words || each.name.includes(words) || each.region.includes(words) || each.range.includes(words),
                            )}
                            rowKey={(each) => `${each.region}/${each.name}`}
                            columns={[
                                { label: "Name", width: "minmax(160px, 1.2fr)", cell: (each) => each.name },
                                { label: "Region", cell: (each) => each.region },
                                { label: "Network", cell: (each) => each.network },
                                { label: "IP range", cell: (each) => <span className="gcp-mono">{each.range}</span> },
                                { label: "Secondary ranges", width: "minmax(140px, 1.2fr)", cell: (each) => each.secondary.join(", ") || "—" },
                                { label: "Private Google Access", cell: (each) => (each.privateGoogleAccess ? "On" : "Off") },
                            ]}
                        />
                    )
                }
            </Loaded>
        </div>
    );
}
