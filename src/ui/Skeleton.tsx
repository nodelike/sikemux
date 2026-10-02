/**
 * Placeholder rows for a list that is still loading.
 *
 * A bare "loading…" string reads as content, then gets replaced by rows of a
 * different height — so the pane jumps. These hold the shape instead.
 */
export function SkeletonRows({ rows = 5, label = "Loading" }: { rows?: number; label?: string }) {
    // Varying widths stop the block from reading as a solid rectangle.
    const widths = ["82%", "64%", "91%", "55%", "74%", "68%", "88%", "60%"];
    return (
        <div className="skel-rows" role="status" aria-label={label} aria-busy="true">
            {Array.from({ length: rows }, (_, i) => (
                <span key={i} className="skel" style={{ width: widths[i % widths.length] }} />
            ))}
        </div>
    );
}
