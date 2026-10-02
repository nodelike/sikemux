import { useResourceEnabled } from "../../plugin-api/resources";
import { SkeletonRows, VirtualLogList } from "../../plugin-api/ui";
import { failureMessage, type RepoRef } from "../api";
import { workflowFileR } from "../resources";

interface Props {
    repo: RepoRef;
    workflowId: string;
    active: boolean;
}

export function WorkflowFile({ repo, workflowId, active }: Props) {
    const file = useResourceEnabled(active, workflowFileR, repo, workflowId);
    const lines = file.data?.text.split("\n") ?? [];

    return (
        <div className="gha-workflow-file">
            {file.status === "loading" && !file.data && <SkeletonRows rows={6} label="Loading the workflow file" />}
            {file.error && !file.data && <div className="gha-side-empty">{failureMessage(file.error)}</div>}
            {file.data && (
                <>
                    <div className="gha-section-label gha-mono">{file.data.path}</div>
                    <VirtualLogList
                        items={lines}
                        className="gha-patch gha-mono"
                        rowClassName="gha-patch-line ctx"
                        estimateSize={18}
                        getItemKey={(_, index) => index}
                        renderRow={(line) => line || " "}
                    />
                </>
            )}
        </div>
    );
}
