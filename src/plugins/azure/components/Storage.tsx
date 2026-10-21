import { useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Seg, SkeletonRows } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { azureApi, type Container, type Scope, type StorageAccount, type Vault } from "../api";
import { ago, region } from "../format";
import { azureVaultsR } from "../resources";

type Which = "accounts" | "vaults";

export function Storage({ accounts, scope, showGroup, active }: { accounts: StorageAccount[]; scope: Scope; showGroup: boolean; active: boolean }) {
    const [which, setWhich] = useState<Which>("accounts");
    const vaults = useResourceEnabled(active && which === "vaults", azureVaultsR, scope);
    return (
        <section className="az-section" aria-label="Storage">
            <div className="az-toolbar">
                <Seg
                    label="Kind"
                    value={which}
                    onChange={setWhich}
                    options={[
                        { value: "accounts", label: "Storage accounts", count: accounts.length },
                        { value: "vaults", label: "Key vaults", count: vaults.data?.length },
                    ]}
                />
            </div>
            {which === "vaults" ? (
                vaults.data === undefined ? (
                    <SkeletonRows rows={4} label="Loading key vaults" />
                ) : (
                    <Vaults vaults={vaults.data} showGroup={showGroup} />
                )
            ) : accounts.length === 0 ? (
                <p className="az-note">No storage accounts here.</p>
            ) : (
                <ul className="az-cards" aria-label="Storage accounts">
                    {accounts.map((account) => (
                        <AccountCard key={account.id} account={account} scope={scope} showGroup={showGroup} />
                    ))}
                </ul>
            )}
        </section>
    );
}

function AccountCard({ account, scope, showGroup }: { account: StorageAccount; scope: Scope; showGroup: boolean }) {
    const [containers, setContainers] = useState<Container[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [open, setOpen] = useState(false);

    const show = async () => {
        setOpen(!open);
        if (containers || !open === false) return;
        try {
            setContainers(await azureApi.containers({ ...scope, group: account.group }, account.name));
        } catch (failure) {
            setError(errorText(failure));
        }
    };

    return (
        <li className="az-card" aria-label={account.name}>
            <header>
                <span className="az-name">{account.name}</span>
                <span className="az-chip az-mono">{account.sku}</span>
                {account.tier ? <span className="az-chip">{account.tier}</span> : null}
                {account.publicAccess ? <span className="az-chip warn">Public blobs allowed</span> : null}
                <span className="az-muted">
                    {showGroup ? `${account.group} · ` : ""}
                    {region(account.location)}
                </span>
            </header>
            <button type="button" className="az-link" aria-expanded={open} onClick={() => void show()}>
                {open ? "Hide containers" : "Show containers"}
            </button>
            {open ? (
                error ? (
                    <p className="az-note az-error">{error}</p>
                ) : containers === null ? (
                    <p className="az-note">Reading containers…</p>
                ) : containers.length === 0 ? (
                    <p className="az-note">No containers yet.</p>
                ) : (
                    <ul className="az-pairs" aria-label={`Containers in ${account.name}`}>
                        {containers.map((container) => (
                            <li key={container.name}>
                                <span className="az-mono">{container.name}</span>
                                {container.publicAccess !== "None" ? <span className="az-chip warn">{container.publicAccess} is public</span> : null}
                                {container.leased ? <span className="az-chip">leased</span> : null}
                                <span className="az-muted">{ago(container.lastChanged)} ago</span>
                            </li>
                        ))}
                    </ul>
                )
            ) : null}
        </li>
    );
}

function Vaults({ vaults, showGroup }: { vaults: Vault[]; showGroup: boolean }) {
    if (vaults.length === 0) return <p className="az-note">No key vaults here.</p>;
    return (
        <table className="az-table" aria-label="Key vaults">
            <thead>
                <tr>
                    <th>Name</th>
                    {showGroup ? <th>Resource group</th> : null}
                    <th>Region</th>
                    <th>Access</th>
                    <th>Protection</th>
                </tr>
            </thead>
            <tbody>
                {vaults.map((vault) => (
                    <tr key={vault.id} aria-label={vault.name}>
                        <td className="az-name">{vault.name}</td>
                        {showGroup ? <td className="az-muted">{vault.group}</td> : null}
                        <td className="az-muted">{region(vault.location)}</td>
                        <td className="az-muted">{vault.rbac ? "Azure RBAC" : "Access policies"}</td>
                        <td className="az-muted">
                            {[vault.softDelete ? "soft delete" : null, vault.purgeProtection ? "purge protection" : null]
                                .filter(Boolean)
                                .join(", ") || "none"}
                        </td>
                    </tr>
                ))}
            </tbody>
        </table>
    );
}
