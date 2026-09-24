import type { Section } from "../state";

const PATHS: Record<Section, string> = {
    services: "M2.5 3.5h11M2.5 8h11M2.5 12.5h11",
    logs: "M3 3.5h10M3 6.5h7M3 9.5h10M3 12.5h5",
    traces: "M2.5 4h6M5 8h8.5M7.5 12h6",
    dashboards: "M2.5 2.5h4.5v5H2.5zM9 2.5h4.5v3H9zM9 7.5h4.5v6H9zM2.5 9.5h4.5v4H2.5z",
};

export function SectionIcon({ section }: { section: Section }) {
    return (
        <svg
            className="sgz-nav-icon"
            width={14}
            height={14}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.4}
            aria-hidden="true">
            <path d={PATHS[section]} />
        </svg>
    );
}
