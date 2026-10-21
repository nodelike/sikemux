import { errorText } from "../../../plugin-api/format";
import { useEffect, useState } from "react";
import { notify, pickFile } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type Bucket, type ObjectPreview, type StorageObject, type Target } from "../api";
import { bucketsR, folderR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Copyable, Facts, keyOf, Loaded, PageHeader, size, Table, visit } from "./parts";
import { Problem } from "./Problem";

export function StoragePage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    if (view.open) return <BucketBrowser paneId={paneId} active={active} target={target} bucket={view.open} />;
    return <Buckets paneId={paneId} active={active} target={target} />;
}

function Buckets({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const buckets = useResourceEnabled(active, bucketsR, ...keyOf(target));
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Buckets">
                <Button onClick={() => visit(consoleUrl("storage/browser", target.project))}>Create bucket in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter buckets" />
            <Loaded handle={buckets} label="buckets" empty={(rows) => (rows.length === 0 ? "No buckets in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Buckets"
                        rows={rows.filter((bucket) => !words || bucket.name.includes(words))}
                        rowKey={(bucket) => bucket.name}
                        onOpen={(bucket: Bucket) => updateGcpView(paneId, { open: bucket.name, prefix: "", filter: "" })}
                        columns={[
                            { label: "Name", width: "minmax(180px, 1.6fr)", cell: (bucket) => <span className="gcp-name">{bucket.name}</span> },
                            { label: "Location", cell: (bucket) => `${bucket.location}${bucket.locationType ? ` (${bucket.locationType})` : ""}` },
                            { label: "Storage class", cell: (bucket) => bucket.storageClass.toLowerCase() },
                            {
                                label: "Public access",
                                cell: (bucket) => (bucket.publicAccessPrevention === "enforced" ? "Not public" : "Subject to object ACLs"),
                            },
                            { label: "Versioning", cell: (bucket) => (bucket.versioning ? "On" : "Off") },
                            { label: "Created", cell: (bucket) => ago(bucket.createdAt) },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

/** `logs/2024/` as its crumbs: `logs/` then `logs/2024/`. */
export function crumbs(prefix: string): { label: string; prefix: string }[] {
    const parts = prefix.split("/").filter(Boolean);
    return parts.map((part, index) => ({ label: part, prefix: `${parts.slice(0, index + 1).join("/")}/` }));
}

function BucketBrowser({ paneId, active, target, bucket }: ProductProps & { bucket: string }) {
    const view = useGcpView(paneId);
    const folder = useResourceEnabled(active, folderR, ...keyOf(target), bucket, view.prefix);
    const [object, setObject] = useState<string | null>(null);
    const go = (prefix: string) => {
        setObject(null);
        updateGcpView(paneId, { prefix });
    };
    const words = view.filter.trim().toLowerCase();
    const [uploading, setUploading] = useState(false);
    const upload = async () => {
        const path = await pickFile(`Upload to ${bucket}/${view.prefix}`);
        if (!path) return;
        setUploading(true);
        try {
            const stored = await gcpApi.objectUpload(target, bucket, view.prefix, path);
            notify("success", `Uploaded ${stored.name}`);
            invalidate((kind) => kind === "gcp.objects");
        } catch (error) {
            notify("error", `Upload: ${errorText(error)}`);
        } finally {
            setUploading(false);
        }
    };
    return (
        <div className="gcp-page">
            <PageHeader
                title={<>Bucket details: {bucket}</>}
                back={{ label: "buckets", go: () => updateGcpView(paneId, { open: null, prefix: "" }) }}>
                <Button primary disabled={uploading} onClick={() => void upload()}>
                    {uploading ? "Uploading…" : "Upload file"}
                </Button>
                <Button onClick={() => visit(consoleUrl(`storage/browser/${bucket}`, target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <nav className="gcp-crumbs" aria-label="Folder">
                <button type="button" className="gcp-link" onClick={() => go("")}>
                    {bucket}
                </button>
                {crumbs(view.prefix).map((crumb) => (
                    <span key={crumb.prefix}>
                        <span className="gcp-muted"> / </span>
                        <button type="button" className="gcp-link" onClick={() => go(crumb.prefix)}>
                            {crumb.label}
                        </button>
                    </span>
                ))}
            </nav>
            <Filter paneId={paneId} placeholder="Filter by name" />
            <div className={object ? "gcp-split" : undefined}>
                <Loaded
                    handle={folder}
                    label="objects"
                    empty={(found) => (found.folders.length + found.objects.length === 0 ? "This folder is empty." : null)}>
                    {(found) => {
                        const rows: ({ folder: string } | { object: StorageObject })[] = [
                            ...found.folders.filter((name) => !words || name.toLowerCase().includes(words)).map((name) => ({ folder: name })),
                            ...found.objects.filter((item) => !words || item.name.toLowerCase().includes(words)).map((item) => ({ object: item })),
                        ];
                        return (
                            <>
                                <Table
                                    label="Objects"
                                    rows={rows}
                                    selected={object}
                                    rowKey={(row) => ("folder" in row ? row.folder : row.object.name)}
                                    onOpen={(row) => ("folder" in row ? go(row.folder) : setObject(row.object.name))}
                                    columns={[
                                        {
                                            label: "Name",
                                            width: "minmax(200px, 2fr)",
                                            cell: (row) =>
                                                "folder" in row ? (
                                                    <span className="gcp-name">📁 {row.folder.slice(view.prefix.length)}</span>
                                                ) : (
                                                    row.object.name.slice(view.prefix.length)
                                                ),
                                        },
                                        { label: "Size", cell: (row) => ("folder" in row ? "—" : size(row.object.size)) },
                                        { label: "Type", cell: (row) => ("folder" in row ? "Folder" : (row.object.contentType ?? "—")) },
                                        { label: "Last modified", cell: (row) => ("folder" in row ? "—" : ago(row.object.updatedAt)) },
                                    ]}
                                />
                                {found.more && <p className="gcp-note">Showing the first 1,000 items in this folder.</p>}
                            </>
                        );
                    }}
                </Loaded>
                {object && <ObjectPanel target={target} bucket={bucket} name={object} onClose={() => setObject(null)} />}
            </div>
        </div>
    );
}

function ObjectPanel({ target, bucket, name, onClose }: { target: Target; bucket: string; name: string; onClose: () => void }) {
    const [preview, setPreview] = useState<ObjectPreview | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [attempt, setAttempt] = useState(0);
    const [saving, setSaving] = useState(false);
    useEffect(() => {
        let current = true;
        setPreview(null);
        setError(null);
        gcpApi
            .objectPreview(target, bucket, name)
            .then((found) => current && setPreview(found))
            .catch((failure: unknown) => current && setError(errorText(failure)));
        return () => {
            current = false;
        };
    }, [target, bucket, name, attempt]);
    const download = () => {
        setSaving(true);
        gcpApi
            .objectDownload(target, bucket, name)
            .then((saved) => notify("success", `Saved to ${saved.path}`))
            .catch((failure: unknown) => notify("error", `Download: ${errorText(failure)}`))
            .finally(() => setSaving(false));
    };
    return (
        <aside className="gcp-panel" aria-label={`Object ${name}`}>
            <header>
                <h2>{name.split("/").pop()}</h2>
                <button type="button" className="gcp-icon-button" aria-label="Close" onClick={onClose}>
                    ✕
                </button>
            </header>
            {error ? (
                <Problem error={error} what="the preview" onRetry={() => setAttempt((n) => n + 1)} />
            ) : !preview ? (
                <SkeletonRows rows={5} label="Loading the object" />
            ) : (
                <>
                    <div className="gcp-log-bar">
                        <Button primary disabled={saving} onClick={download}>
                            {saving ? "Downloading…" : "Download"}
                        </Button>
                    </div>
                    <Facts
                        items={[
                            ["gsutil URI", <Copyable key="uri" text={preview.uri} what="the URI" />],
                            ["Size", size(preview.size)],
                            ["Type", preview.contentType],
                            ["Storage class", preview.storageClass?.toLowerCase()],
                            ["Last modified", ago(preview.updatedAt)],
                        ]}
                    />
                    {preview.preview !== null ? (
                        <>
                            {preview.trimmed && <p className="gcp-note">Showing the first 64 KiB.</p>}
                            <pre className="gcp-terminal">{preview.preview}</pre>
                        </>
                    ) : (
                        <p className="gcp-note">This object is not text, so it has no preview.</p>
                    )}
                </>
            )}
        </aside>
    );
}
