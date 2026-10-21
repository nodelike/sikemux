export function AzureMark({ size = 16, className }: { size?: number; className?: string }) {
    return (
        <svg width={size} height={size} viewBox="0 0 96 96" fill="currentColor" className={className} aria-hidden="true">
            <path d="M33.3 8h26.6L32.3 90.3a4.2 4.2 0 0 1-4 2.9H7.2a4.2 4.2 0 0 1-4-5.6L29.3 10.9a4.2 4.2 0 0 1 4-2.9Z" opacity=".75" />
            <path d="M71.9 63.1H29.6a2 2 0 0 0-1.3 3.4l27.2 25.4a4.2 4.2 0 0 0 2.9 1.2h23.9Z" />
            <path
                d="M33.3 8a4.2 4.2 0 0 0-4 2.9L3.3 87.5a4.1 4.1 0 0 0 3.9 5.7h21.4a4.5 4.5 0 0 0 3.5-2.9l5.2-15.3 18.5 17.3a4.3 4.3 0 0 0 2.7.9h23.8L72.7 63.1H42.1l18.7-55.1Z"
                opacity=".9"
            />
            <path d="M66.7 10.9a4.2 4.2 0 0 0-4-2.9H33.6a4.2 4.2 0 0 1 4 2.9l26.1 76.7a4.2 4.2 0 0 1-4 5.6h29.1a4.2 4.2 0 0 0 4-5.6Z" opacity=".6" />
        </svg>
    );
}
