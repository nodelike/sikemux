import type { ReactNode } from "react";
import type { ResourceHandle } from "../state/resources";
import { EmptyState } from "../ui/Panel";
import { SkeletonRows } from "../ui/Skeleton";

type Handle<T> = Pick<ResourceHandle<T>, "data" | "status" | "error">;

/** What stands in for a read that has no data yet: rows loading, or why it could not be read. */
export function Pending({ handle, loading, rows = 6 }: { handle: Handle<unknown>; loading: string; rows?: number }) {
    return handle.status === "error" ? <EmptyState message={handle.error} tone="error" /> : <SkeletonRows rows={rows} label={loading} />;
}

/** A read's data once it has some, and what stands in for it until then. */
export function Loaded<T>({
    handle,
    loading,
    rows,
    empty,
    children,
}: {
    handle: Handle<T>;
    /** Said while it loads, such as "Loading issues". */
    loading: string;
    rows?: number;
    /** What to say when the data holds nothing to show. */
    empty?: (data: T) => string | null;
    children: (data: T) => ReactNode;
}) {
    if (handle.data === undefined) return <Pending handle={handle} loading={loading} rows={rows} />;
    const nothing = empty?.(handle.data);
    return nothing ? <EmptyState message={nothing} /> : children(handle.data);
}
