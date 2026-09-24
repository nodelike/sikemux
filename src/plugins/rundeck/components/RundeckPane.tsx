import { useMemo, useRef } from "react";
import * as cmd from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { rndStatusR } from "../resources";
import { EmptyState, IconWarning } from "../../../plugin-api/ui";
import { RundeckBreadcrumb } from "./RundeckBreadcrumb";
import { RundeckLogin } from "./RundeckLogin";
import { RundeckMatrix } from "./RundeckMatrix";
import { RundeckProjectTree } from "./RundeckProjectTree";
import { RundeckService } from "./RundeckService";
import { RundeckDeploy } from "./RundeckDeploy";
import { RundeckExecution } from "./RundeckExecution";
import "../rundeck.css";

interface Props {
    paneId: string;
    active: boolean;
}

export function RundeckPane({ paneId, active }: Props) {
    const view = cmd.useRundeckView(paneId);
    const status = useResourceEnabled(active, rndStatusR);
    const treeHidden = cmd.rundeckSettings.useSelect((s) => s.treeHidden);

    const top = useMemo(() => view.stack[view.stack.length - 1] ?? { kind: "matrix" as const }, [view.stack]);

    const body = useMemo(() => {
        if (status.status === "loading" && !status.data) {
            return <RundeckLoading />;
        }
        if (status.data && (!status.data.configured || (!status.data.ok && status.data.auth_failed))) {
            return (
                <RundeckLogin
                    key={status.data.configured ? "expired" : "new"}
                    initialUrl={status.data.url}
                    initialUser={status.data.user}
                    initialAllowInsecurePrivateHttp={status.data.allow_insecure_private_http}
                    notice={status.data.configured ? (status.data.message ?? "Authentication failed") : undefined}
                    onDone={() => status.refresh()}
                />
            );
        }
        if (status.data && !status.data.ok) {
            return <RundeckStatusError message={status.data.message ?? "Rundeck connection failed"} onRetry={() => status.refresh()} />;
        }
        if (top.kind === "matrix") return <RundeckMatrix paneId={paneId} active={active} />;
        if (top.kind === "service") return <RundeckService key={top.jobId} paneId={paneId} level={top} active={active} />;
        if (top.kind === "deploy") return <RundeckDeploy key={top.jobId} paneId={paneId} level={top} active={active} />;
        if (top.kind === "execution") return <RundeckExecution key={top.executionId} paneId={paneId} level={top} active={active} />;
        return null;
    }, [paneId, status, top, active]);

    const signedIn = !!status.data && status.data.configured && status.data.ok;

    return (
        <div className="rnd-pane" data-active={active ? "1" : "0"}>
            <RundeckBreadcrumb paneId={paneId} status={status.data ?? null} signedIn={signedIn} onSignedOut={() => void status.refresh()} />
            <div className="rnd-cols">
                {signedIn && !treeHidden && (
                    <>
                        <RundeckProjectTree paneId={paneId} active={active} />
                        <TreeResizer />
                    </>
                )}
                <div className="rnd-body">{body}</div>
            </div>
        </div>
    );
}

function TreeResizer() {
    const drag = useRef<{ x: number; width: number } | null>(null);
    return (
        <div
            className="rnd-tree-resizer"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize project tree"
            onPointerDown={(event) => {
                event.preventDefault();
                event.currentTarget.setPointerCapture(event.pointerId);
                drag.current = { x: event.clientX, width: cmd.useRundeck.getState().treeWidth };
            }}
            onPointerMove={(event) => {
                if (!drag.current) return;
                cmd.setTreeWidth(drag.current.width + event.clientX - drag.current.x);
            }}
            onPointerUp={(event) => {
                drag.current = null;
                event.currentTarget.releasePointerCapture(event.pointerId);
            }}
            onPointerCancel={() => {
                drag.current = null;
            }}
        />
    );
}

function RundeckStatusError({ message, onRetry }: { message: string; onRetry: () => void }) {
    return (
        <EmptyState
            tone="error"
            icon={<IconWarning size={14} />}
            title="Can't reach Rundeck"
            message={message}
            action={{ label: "Retry", onClick: onRetry }}
        />
    );
}

function RundeckLoading() {
    return (
        <div className="rnd-loading">
            <span className="rnd-spinner" />
            <span>checking rundeck…</span>
        </div>
    );
}
