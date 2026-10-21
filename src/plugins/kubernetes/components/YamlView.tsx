import { copyWithNotice } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Pending } from "../../../plugin-api/ui";
import type { Scope } from "../api";
import { kubeYamlR } from "../resources";

/** An object's YAML to read, as kubectl writes it, less what the pane never shows. */
export function YamlView({ scope, kind, name, active }: { scope: Scope; kind: string; name: string; active: boolean }) {
    const yaml = useResourceEnabled(active, kubeYamlR, scope, kind, name);
    if (yaml.data === undefined) return <Pending handle={yaml} loading="Reading the YAML" rows={8} />;
    const lines = yaml.data.replace(/\n$/, "").split("\n");
    return (
        <section className="k8s-yaml" aria-label={`${kind} ${name} YAML`}>
            <div className="k8s-toolbar">
                <span className="k8s-muted">Env values, ConfigMap data and the copy kubectl apply keeps are left out.</span>
                <button type="button" className="k8s-button k8s-yaml-copy" onClick={() => copyWithNotice(yaml.data ?? "", "The YAML")}>
                    Copy
                </button>
            </div>
            <pre className="k8s-yaml-text">
                {lines.map((line, index) => (
                    <span key={index}>
                        <span className="k8s-yaml-no" aria-hidden="true">
                            {index + 1}
                        </span>
                        {line || " "}
                    </span>
                ))}
            </pre>
        </section>
    );
}
