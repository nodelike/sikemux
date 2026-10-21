import { useResourceEnabled } from "../../../plugin-api/resources";
import type { DnsZone } from "../api";
import { dnsRecordsR, dnsZonesR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { Button, consoleUrl, Facts, keyOf, Loaded, PageHeader, Table, visit } from "./parts";

export function DnsPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const zones = useResourceEnabled(active, dnsZonesR, ...keyOf(target));
    const zone = zones.data?.find((each) => each.name === view.open);
    if (zone) return <ZonePage paneId={paneId} active={active} target={target} zone={zone} />;
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Cloud DNS">
                <Button onClick={() => visit(consoleUrl("net-services/dns/zones", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter zones" />
            <Loaded handle={zones} label="zones" empty={(rows) => (rows.length === 0 ? "No Cloud DNS zones in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Zones"
                        rows={rows.filter((each) => !words || each.name.includes(words) || each.dnsName.includes(words))}
                        rowKey={(each) => each.name}
                        onOpen={(each: DnsZone) => updateGcpView(paneId, { open: each.name, filter: "" })}
                        columns={[
                            { label: "Zone name", width: "minmax(140px, 1.2fr)", cell: (each) => <span className="gcp-name">{each.name}</span> },
                            { label: "DNS name", width: "minmax(160px, 1.4fr)", cell: (each) => each.dnsName },
                            { label: "Type", cell: (each) => (each.visibility === "private" ? "Private" : "Public") },
                            { label: "DNSSEC", cell: (each) => (each.dnssec ? "On" : "Off") },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

function ZonePage({ paneId, active, target, zone }: ProductProps & { zone: DnsZone }) {
    const view = useGcpView(paneId);
    const records = useResourceEnabled(active, dnsRecordsR, ...keyOf(target), zone.name);
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title={zone.dnsName} back={{ label: "zones", go: () => updateGcpView(paneId, { open: null, filter: "" }) }}>
                <Button onClick={() => visit(consoleUrl(`net-services/dns/zones/${zone.name}/details`, target.project))}>
                    Edit records in Cloud console
                </Button>
            </PageHeader>
            <Facts
                items={[
                    ["Zone", zone.name],
                    ["Type", zone.visibility === "private" ? "Private" : "Public"],
                    ["DNSSEC", zone.dnssec ? "On" : "Off"],
                    ["Name servers", zone.nameServers?.join(", ")],
                ]}
            />
            <Filter paneId={paneId} placeholder="Filter records" />
            <Loaded handle={records} label="records" empty={(rows) => (rows.length === 0 ? "No records." : null)}>
                {(rows) => (
                    <Table
                        label="Records"
                        rows={rows.filter(
                            (each) =>
                                !words ||
                                each.name.includes(words) ||
                                each.type.toLowerCase() === words ||
                                each.data.some((value) => value.includes(words)),
                        )}
                        rowKey={(each) => `${each.name}-${each.type}`}
                        columns={[
                            { label: "DNS name", width: "minmax(180px, 1.6fr)", cell: (each) => each.name },
                            { label: "Type", width: "70px", cell: (each) => each.type },
                            { label: "TTL", width: "70px", cell: (each) => each.ttl ?? "—" },
                            {
                                label: "Data",
                                width: "minmax(200px, 2fr)",
                                cell: (each) => (
                                    <span className="gcp-mono" title={each.data.join("\n")}>
                                        {each.data.join(", ")}
                                    </span>
                                ),
                            },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}
