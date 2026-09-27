import type { ReactNode } from "react";
import type { AwsService } from "../state";

function Glyph({ size = 15, children }: { size?: number; children: ReactNode }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.3}
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true">
            {children}
        </svg>
    );
}

const SERVICE_GLYPHS: Record<AwsService, ReactNode> = {
    ecs: (
        <>
            <path d="M8 1.8 13.5 5v6L8 14.2 2.5 11V5z" />
            <path d="M2.5 5 8 8.2 13.5 5M8 8.2v6" />
        </>
    ),
    ec2: (
        <>
            <rect x="4" y="4" width="8" height="8" rx="1.5" />
            <path d="M6.5 1.5V4M9.5 1.5V4M6.5 12v2.5M9.5 12v2.5M1.5 6.5H4M1.5 9.5H4M12 6.5h2.5M12 9.5h2.5" />
        </>
    ),
    lambda: <path d="M3.5 2.5h2.2l6.8 11H14M8 7.2 4 13.5" />,
    sqs: (
        <>
            <rect x="1.8" y="4.5" width="3" height="7" rx="1" />
            <rect x="6.5" y="4.5" width="3" height="7" rx="1" />
            <path d="M11.5 8h3M13 6.5 14.5 8 13 9.5" />
        </>
    ),
    billing: (
        <>
            <path d="M3.5 1.8h9v12.4l-1.5-1-1.5 1-1.5-1-1.5 1-1.5-1-1.5 1z" />
            <path d="M6 5.5h4M6 8h4M6 10.5h2" />
        </>
    ),
    s3: (
        <>
            <ellipse cx="8" cy="4" rx="5.5" ry="2" />
            <path d="M2.5 4 4 12.5c.3 1 1.9 1.7 4 1.7s3.7-.7 4-1.7L13.5 4" />
        </>
    ),
};

export function ServiceGlyph({ service, size }: { service: AwsService; size?: number }) {
    return <Glyph size={size}>{SERVICE_GLYPHS[service]}</Glyph>;
}

export function IconExternal({ size = 14 }: { size?: number }) {
    return (
        <Glyph size={size}>
            <path d="M9 2.5h4.5V7M13.5 2.5 7.5 8.5M11.5 9.5v3.5h-9v-9H6" />
        </Glyph>
    );
}

export function IconLogs({ size = 14 }: { size?: number }) {
    return (
        <Glyph size={size}>
            <path d="M2.5 4h11M2.5 8h7M2.5 12h9" />
        </Glyph>
    );
}

export function IconTasks({ size = 14 }: { size?: number }) {
    return (
        <Glyph size={size}>
            <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1" />
            <rect x="9" y="2.5" width="4.5" height="4.5" rx="1" />
            <rect x="2.5" y="9" width="4.5" height="4.5" rx="1" />
            <rect x="9" y="9" width="4.5" height="4.5" rx="1" />
        </Glyph>
    );
}
