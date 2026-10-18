import { useId } from "react";

/** Jira's mark: three stacked tiles, each pointing to the next. */
export function JiraMark({ size = 16, className, mono }: { size?: number; className?: string; mono?: boolean }) {
    const id = useId();
    const fade = mono ? "currentColor" : `url(#${id})`;
    return (
        <svg width={size} height={size} viewBox="0 0 256 256" className={className} aria-hidden="true">
            {!mono && (
                <defs>
                    <linearGradient id={id} x1="0.98" y1="0" x2="0.59" y2="0.41">
                        <stop offset="0.18" stopColor="#0052CC" />
                        <stop offset="1" stopColor="#2684FF" />
                    </linearGradient>
                </defs>
            )}
            <path
                fill={mono ? "currentColor" : "#2684FF"}
                d="M244.66 0 L121.71 0 C121.71 14.72 127.55 28.84 137.96 39.25 C148.37 49.65 162.49 55.5 177.21 55.5 L199.86 55.5 L199.86 77.37 C199.88 107.99 224.7 132.82 255.32 132.84 L255.32 10.67 C255.32 4.78 250.55 0 244.66 0 Z"
            />
            <path
                fill={fade}
                d="M183.82 61.26 L60.87 61.26 C60.89 91.89 85.71 116.71 116.34 116.73 L138.99 116.73 L138.99 138.67 C139.03 169.29 163.86 194.1 194.49 194.1 L194.49 71.93 C194.49 66.04 189.71 61.26 183.82 61.26 Z"
            />
            <path
                fill={fade}
                d="M122.95 122.49 L0 122.49 C0 153.14 24.85 177.99 55.5 177.99 L78.22 177.99 L78.22 199.86 C78.24 230.46 103.02 255.27 133.62 255.32 L133.62 133.16 C133.62 127.26 128.84 122.49 122.95 122.49 Z"
            />
        </svg>
    );
}
