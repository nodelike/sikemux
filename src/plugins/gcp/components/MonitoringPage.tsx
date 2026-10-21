import { useResourceEnabled } from "../../../plugin-api/resources";
import { gcpApi } from "../api";
import { alertingR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Button, consoleUrl, keyOf, Loaded, PageHeader, Status, Table, Tabs, useAction, visit } from "./parts";

const LISTS = [
    { value: "policies", label: "Alert policies" },
    { value: "uptime", label: "Uptime checks" },
    { value: "channels", label: "Notification channels" },
] as const;

export function MonitoringPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = LISTS.find((each) => each.value === view.list)?.value ?? "policies";
    const alerting = useResourceEnabled(active, alertingR, ...keyOf(target));
    const action = useAction();
    return (
        <div className="gcp-page">
            <PageHeader title="Monitoring">
                <Button onClick={() => visit(consoleUrl("monitoring/alerting", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <Tabs label="Monitoring" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value })} />
            <Loaded handle={alerting} label="alerting" empty={() => null}>
                {(data) =>
                    list === "policies" ? (
                        data.policies.length === 0 ? (
                            <p className="gcp-note">No alert policies. Nothing in this project tells anyone when it breaks.</p>
                        ) : (
                            <Table
                                label="Alert policies"
                                rows={data.policies}
                                rowKey={(policy) => policy.id}
                                columns={[
                                    { label: "", width: "24px", cell: (policy) => <Status state={policy.enabled ? "ENABLED" : "DISABLED"} /> },
                                    {
                                        label: "Name",
                                        width: "minmax(160px, 1.2fr)",
                                        cell: (policy) => <span title={policy.documentation ?? undefined}>{policy.name}</span>,
                                    },
                                    {
                                        label: "Conditions",
                                        width: "minmax(240px, 2.4fr)",
                                        cell: (policy) => (
                                            <span title={policy.conditions.map((each) => each.summary).join("\n")}>
                                                {policy.conditions.map((each) => each.summary).join("; ")}
                                            </span>
                                        ),
                                    },
                                    {
                                        label: "Notifies",
                                        width: "80px",
                                        cell: (policy) =>
                                            policy.channels === 0 ? (
                                                <span className="gcp-error-text">No one</span>
                                            ) : (
                                                `${policy.channels} channel${policy.channels === 1 ? "" : "s"}`
                                            ),
                                    },
                                    {
                                        label: "",
                                        width: "100px",
                                        align: "end",
                                        cell: (policy) => (
                                            <Button
                                                danger={policy.enabled}
                                                disabled={action.busy}
                                                onClick={() =>
                                                    void action.run(
                                                        {
                                                            title: `${policy.enabled ? "Turn off" : "Turn on"} ${policy.name}?`,
                                                            body: policy.enabled
                                                                ? "It stops opening incidents and notifying anyone until turned on again."
                                                                : "It opens incidents and notifies its channels again.",
                                                            confirmLabel: policy.enabled ? "Turn off" : "Turn on",
                                                            destructive: policy.enabled,
                                                        },
                                                        `${policy.name} is ${policy.enabled ? "off" : "on"}`,
                                                        () => gcpApi.alertPolicyToggle(target, policy.id, !policy.enabled),
                                                    )
                                                }>
                                                {policy.enabled ? "Turn off" : "Turn on"}
                                            </Button>
                                        ),
                                    },
                                ]}
                            />
                        )
                    ) : list === "uptime" ? (
                        <Table
                            label="Uptime checks"
                            rows={data.uptimeChecks}
                            rowKey={(check) => check.id}
                            columns={[
                                { label: "Name", width: "minmax(160px, 1.2fr)", cell: (check) => check.name },
                                { label: "Checks", width: "minmax(200px, 2fr)", cell: (check) => <span className="gcp-mono">{check.target}</span> },
                                { label: "Every", cell: (check) => check.period ?? "—" },
                                { label: "Regions", cell: (check) => check.regions?.length ?? "All" },
                            ]}
                        />
                    ) : (
                        <Table
                            label="Notification channels"
                            rows={data.channels}
                            rowKey={(channel) => `${channel.type}-${channel.name}`}
                            columns={[
                                { label: "", width: "24px", cell: (channel) => <Status state={channel.enabled ? "ENABLED" : "DISABLED"} /> },
                                { label: "Name", width: "minmax(180px, 1.6fr)", cell: (channel) => channel.name },
                                { label: "Type", cell: (channel) => channel.type },
                                { label: "Verified", cell: (channel) => (channel.verified ? "Yes" : <span className="gcp-error-text">No</span>) },
                            ]}
                        />
                    )
                }
            </Loaded>
        </div>
    );
}
