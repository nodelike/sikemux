import { copyWithNotice } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import type { DockerImage, Repository } from "../api";
import { repositoriesR, repositoryR } from "../resources";
import { splitOpen, updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, keyOf, Loaded, PageHeader, size, Table, visit } from "./parts";

export function RegistryPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const opened = splitOpen(view.open);
    const repositories = useResourceEnabled(active && !opened, repositoriesR, ...keyOf(target));
    if (opened) return <RepositoryPage paneId={paneId} active={active} target={target} location={opened.location} name={opened.name} />;
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Repositories">
                <Button onClick={() => visit(consoleUrl("artifacts", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter repositories" />
            <Loaded
                handle={repositories}
                label="repositories"
                empty={(rows) => (rows.length === 0 ? "No Artifact Registry repositories in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Repositories"
                        rows={rows.filter((repo) => !words || repo.name.toLowerCase().includes(words))}
                        rowKey={(repo) => `${repo.location}/${repo.name}`}
                        onOpen={(repo: Repository) => updateGcpView(paneId, { open: `${repo.location}/${repo.name}`, filter: "" })}
                        columns={[
                            { label: "Name", width: "minmax(160px, 1.4fr)", cell: (repo) => <span className="gcp-name">{repo.name}</span> },
                            { label: "Format", cell: (repo) => repo.format.charAt(0) + repo.format.slice(1).toLowerCase() },
                            { label: "Location", cell: (repo) => repo.location },
                            { label: "Size", cell: (repo) => size(repo.sizeBytes) },
                            { label: "Updated", cell: (repo) => ago(repo.updatedAt) },
                            {
                                label: "Description",
                                width: "minmax(140px, 1.4fr)",
                                cell: (repo) => <span className="gcp-muted">{repo.description ?? ""}</span>,
                            },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

function RepositoryPage({ paneId, active, target, location, name }: ProductProps & { location: string; name: string }) {
    const view = useGcpView(paneId);
    const contents = useResourceEnabled(active, repositoryR, ...keyOf(target), location, name);
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title={name} back={{ label: "repositories", go: () => updateGcpView(paneId, { open: null, filter: "" }) }}>
                <Button onClick={() => visit(consoleUrl(`artifacts/docker/${target.project}/${location}/${name}`, target.project))}>
                    Open in Cloud console
                </Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter images or tags" />
            <Loaded
                handle={contents}
                label="images"
                empty={(found) => (("images" in found ? found.images : found.packages).length === 0 ? "This repository is empty." : null)}>
                {(found) =>
                    "images" in found ? (
                        <Table
                            label="Images"
                            rows={found.images.filter(
                                (image) =>
                                    !words ||
                                    image.image.toLowerCase().includes(words) ||
                                    image.tags.some((tag) => tag.toLowerCase().includes(words)),
                            )}
                            rowKey={(image) => image.digest}
                            onOpen={(image: DockerImage) => copyWithNotice(image.uri, "the image address")}
                            columns={[
                                { label: "Image", width: "minmax(140px, 1.2fr)", cell: (image) => <span className="gcp-name">{image.image}</span> },
                                {
                                    label: "Tags",
                                    width: "minmax(120px, 1.2fr)",
                                    cell: (image) =>
                                        image.tags.length > 0 ? (
                                            image.tags.map((tag) => (
                                                <span key={tag} className="gcp-chip">
                                                    {tag}
                                                </span>
                                            ))
                                        ) : (
                                            <span className="gcp-muted">untagged</span>
                                        ),
                                },
                                {
                                    label: "Digest",
                                    cell: (image) => <span className="gcp-mono gcp-muted">{image.digest.replace("sha256:", "").slice(0, 12)}</span>,
                                },
                                { label: "Size", cell: (image) => size(image.sizeBytes) },
                                { label: "Uploaded", cell: (image) => ago(image.uploadedAt) },
                            ]}
                        />
                    ) : (
                        <Table
                            label="Packages"
                            rows={found.packages.filter((pkg) => !words || pkg.name.toLowerCase().includes(words))}
                            rowKey={(pkg) => pkg.name}
                            columns={[
                                { label: "Package", width: "minmax(180px, 2fr)", cell: (pkg) => pkg.name },
                                { label: "Updated", cell: (pkg) => ago(pkg.updatedAt) },
                            ]}
                        />
                    )
                }
            </Loaded>
            <p className="gcp-note">Click an image to copy its address with its digest, which pins the exact build.</p>
        </div>
    );
}
