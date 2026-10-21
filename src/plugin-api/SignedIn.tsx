import type { ReactNode } from "react";
import { useResourceEnabled } from "../state/resources";
import { SkeletonRows } from "../ui/Skeleton";
import type { Service, ServiceStatus } from "./service";

/** A service's pane once the person is signed in to it, and its sign-in form until then. */
export function SignedIn<Status extends ServiceStatus>({
    service,
    active,
    signIn,
    children,
}: {
    service: Service<Status>;
    active: boolean;
    signIn: (status: Status | undefined) => ReactNode;
    children: (status: Status) => ReactNode;
}) {
    const status = useResourceEnabled(active, service.statusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label={`Connecting to ${service.name}`} />;
    if (!status.data?.configured || status.data.authFailed) return signIn(status.data);
    return children(status.data);
}
