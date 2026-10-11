/** A database drum: a stack of three discs. */
export function DatabaseMark({ size = 16, className }: { size?: number; className?: string }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 24 24"
            className={className}
            fill="none"
            stroke="#3B9EE8"
            strokeWidth="1.8"
            strokeLinecap="round"
            aria-hidden="true">
            <ellipse cx="12" cy="5.5" rx="7.5" ry="3" />
            <path d="M4.5 5.5v6.5c0 1.66 3.36 3 7.5 3s7.5-1.34 7.5-3V5.5" />
            <path d="M4.5 12v6.5c0 1.66 3.36 3 7.5 3s7.5-1.34 7.5-3V12" />
        </svg>
    );
}
