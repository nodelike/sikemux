/** The Kubernetes helm wheel, drawn in the current text colour. */
export function KubernetesMark({ size = 16, className }: { size?: number; className?: string }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            className={className}
            aria-hidden="true">
            <path d="M12 2.6 20.2 6.6 22.2 15.4 16.6 22.4H7.4L1.8 15.4 3.8 6.6Z" strokeLinejoin="round" />
            <circle cx="12" cy="12.6" r="3.2" />
            <path
                d="M12 5.6v3.8M12 15.8v3.6M5.4 10.6l3.6 1.2M15 13.4l3.6 1.2M7.8 18.2l2.3-2.9M13.9 9.9l2.3-2.9M7.8 7l2.3 2.9M13.9 15.3l2.3 2.9"
                strokeLinecap="round"
            />
        </svg>
    );
}
