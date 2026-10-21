/** Docker's whale carrying its containers, drawn in the current text colour. */
export function DockerMark({ size = 16, className }: { size?: number; className?: string }) {
    return (
        <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" className={className} aria-hidden="true">
            <path d="M3.5 9.5h2.4v2.2H3.5zm2.8 0h2.4v2.2H6.3zm2.8 0h2.4v2.2H9.1zm2.8 0h2.4v2.2h-2.4zM6.3 6.8h2.4V9H6.3zm2.8 0h2.4V9H9.1zm2.8 0h2.4V9h-2.4zm0-2.7h2.4v2.2h-2.4zm2.8 5.4h2.4v2.2h-2.4z" />
            <path d="M23.3 10.6c-.6-.4-1.9-.5-2.9-.3-.1-1-.7-1.9-1.6-2.6l-.5-.4-.4.5c-.5.8-.7 2-.6 2.9 0 .4.2.9.4 1.2-.6.3-1.4.4-2.1.4H1c-.2 1.4 0 3.1.9 4.4.9 1.3 2.4 2.1 4.6 2.1 4.6 0 8.1-2.1 9.7-6.6.7 0 2.1 0 2.8-1.4l.2-.4-.9-.4Z" />
        </svg>
    );
}
