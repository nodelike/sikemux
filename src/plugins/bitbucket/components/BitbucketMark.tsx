import { useId } from "react";

export function BitbucketMark({ size = 16, className, mono }: { size?: number; className?: string; mono?: boolean }) {
    const id = useId();
    return (
        <svg width={size} height={size} viewBox="0 -12.5 256 256" className={className} aria-hidden="true">
            {!mono && (
                <defs>
                    <linearGradient id={id} x1="1.09" y1="0.14" x2="0.47" y2="0.79">
                        <stop offset="0.18" stopColor="#0052CC" />
                        <stop offset="1" stopColor="#2684FF" />
                    </linearGradient>
                </defs>
            )}
            <path
                fill={mono ? "currentColor" : "#2684FF"}
                d="M8.31 0 C5.89 -0.03 3.57 1.01 1.99 2.85 C0.41 4.68 -0.28 7.12 0.11 9.52 L34.92 220.89 C35.82 226.23 40.42 230.15 45.83 230.2 L212.87 230.2 C216.94 230.25 220.43 227.32 221.08 223.31 L255.89 9.56 C256.28 7.16 255.59 4.72 254.01 2.89 C252.43 1.05 250.11 0.01 247.69 0.04 L8.31 0 Z M154.92 152.77 L101.61 152.77 L87.17 77.35 L167.84 77.35 L154.92 152.77 Z"
            />
            {!mono && (
                <path
                    fill={`url(#${id})`}
                    d="M244.61 77.24 L167.69 77.24 L154.79 152.6 L101.51 152.6 L38.61 227.26 C40.6 228.99 43.15 229.95 45.78 229.97 L212.73 229.97 C216.79 230.02 220.28 227.09 220.93 223.08 L244.61 77.24 Z"
                />
            )}
        </svg>
    );
}
