export function VercelMark({ size = 16, className }: { size?: number; className?: string }) {
    return (
        <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
            <path d="m12 1.608 12 20.784H0Z" />
        </svg>
    );
}
