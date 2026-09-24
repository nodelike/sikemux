import type { ResourceHandle } from "../../../plugin-api/resources";
import { IconRefresh } from "../../../plugin-api/ui";

export function AwsRefresh<T>({ handle }: { handle: ResourceHandle<T> }) {
    const busy = handle.status === "loading";
    return (
        <button
            className={`aws-refresh${busy ? " busy" : ""}`}
            onClick={() => void handle.refresh()}
            disabled={busy}
            title={busy ? "refreshing…" : "refresh from AWS"}>
            <IconRefresh size={12} />
        </button>
    );
}
