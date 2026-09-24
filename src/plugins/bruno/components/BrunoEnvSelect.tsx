import { IconChevron, IconShield } from "../../../plugin-api/ui";
import type { BruEnv } from "../lib/types";
import { brunoSetSecret, brunoToggleSecrets, openPalette } from "../state";

interface Props {
    paneId: string;
    envs: BruEnv[];
    showCollection: boolean;
    selected: string | null;
    secretNames: string[];
    secretVars: Record<string, string>;
    secretsOpen: boolean;
}

export function BrunoEnvSelect({ paneId, envs, showCollection, selected, secretNames, secretVars, secretsOpen }: Props) {
    const active = selected ? envs.find((e) => e.id === selected) : undefined;
    const label = active ? (showCollection ? `${active.collectionName}/${active.name}` : active.name) : "No environment";

    return (
        <div className="bruno-env">
            <button type="button" className="dd-btn bruno-env-dd" title="Environment (⌥E)" onClick={() => openPalette("environmentPalette")}>
                <span className="dd-val">{label}</span>
                <IconChevron size={9} className="dd-chev" />
            </button>
            {secretNames.length > 0 && (
                <button
                    className={`bruno-secrets-btn${secretsOpen ? " active" : ""}`}
                    title="Secret variables"
                    onClick={() => brunoToggleSecrets(paneId)}>
                    <IconShield size={12} />
                    secrets
                </button>
            )}
            {secretsOpen && secretNames.length > 0 && (
                <div className="bruno-secrets-pop">
                    <div className="bruno-secrets-head">Secret variables{selected ? ` · ${selected}` : ""}</div>
                    {secretNames.map((name) => (
                        <label key={name} className="bruno-secret-row">
                            <span className="bruno-secret-name">{name}</span>
                            <input
                                type="password"
                                className="bruno-input"
                                value={secretVars[name] ?? ""}
                                placeholder="not set"
                                autoComplete="off"
                                spellCheck={false}
                                onChange={(e) => brunoSetSecret(paneId, name, e.target.value)}
                            />
                        </label>
                    ))}
                    <div className="bruno-secrets-foot">stored locally on this machine, not written to .bru files</div>
                </div>
            )}
        </div>
    );
}
