import { useState } from "react";
import { Dropdown } from "../../../plugin-api/ui";
import type { LogResource } from "../api";
import type { ProductProps } from "./GcpPane";
import { LogStream } from "./LogStream";
import { Button, consoleUrl, PageHeader, visit } from "./parts";

const RESOURCES: { value: LogResource | ""; label: string }[] = [
    { value: "", label: "All resources" },
    { value: "runService", label: "Cloud Run service" },
    { value: "runJob", label: "Cloud Run job" },
    { value: "function", label: "Cloud Function" },
    { value: "instance", label: "VM instance" },
    { value: "gkeCluster", label: "Kubernetes container" },
    { value: "build", label: "Cloud Build" },
    { value: "sql", label: "Cloud SQL" },
];

/** Logs Explorer: any resource's logs, a raw filter beside the searches, and streaming. */
export function LoggingPage({ active, target }: ProductProps) {
    const [resource, setResource] = useState<LogResource | "">("");
    const [name, setName] = useState("");
    const [filter, setFilter] = useState("");
    const [applied, setApplied] = useState({ name: "", filter: "" });
    return (
        <div className="gcp-page">
            <PageHeader title="Logs Explorer">
                <Button onClick={() => visit(consoleUrl("logs/query", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <div className="gcp-query">
                <Dropdown label="Resource" value={resource} options={RESOURCES} onChange={(value) => setResource(value as LogResource | "")} />
                {resource && (
                    <input
                        className="gcp-input"
                        value={name}
                        onChange={(event) => setName(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter") setApplied({ ...applied, name: name.trim() });
                        }}
                        placeholder="Name, such as a service"
                        aria-label="Resource name"
                        spellCheck={false}
                    />
                )}
                <textarea
                    className="gcp-input gcp-filter"
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) setApplied({ name: name.trim(), filter: filter.trim() });
                    }}
                    placeholder={'Query, in the Logging query language: httpRequest.status>=500 AND jsonPayload.user="ana"'}
                    aria-label="Logging query"
                    spellCheck={false}
                />
                <Button primary onClick={() => setApplied({ name: name.trim(), filter: filter.trim() })}>
                    Run query
                </Button>
            </div>
            <LogStream
                active={active}
                target={target}
                query={{ resource: resource || null, name: resource ? applied.name || null : null }}
                extraFilter={applied.filter}
            />
        </div>
    );
}
