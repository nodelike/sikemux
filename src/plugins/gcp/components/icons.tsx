import type { Product } from "../state";

/** Google Cloud's mark: the cloud in Google's four colours. */
export function GcpMark({ size = 16 }: { size?: number }) {
    return (
        <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
            <path fill="#EA4335" d="M15.2 7.3h.7l2-2 .1-.9A9 9 0 0 0 3.4 8.8l.7-.1 4-.7.3-.3a5 5 0 0 1 6.8-.4z" />
            <path
                fill="#4285F4"
                d="M20.8 8.8a9 9 0 0 0-2.7-4.4l-2.8 2.8a5 5 0 0 1 1.8 4v.5a2.5 2.5 0 1 1 0 5h-5l-.5.5v3l.5.5h5a6.5 6.5 0 0 0 3.7-11.9"
            />
            <path fill="#34A853" d="M7 20.7h5v-4H7a2.5 2.5 0 0 1-1-.2l-.7.2-2 2-.2.7a6.5 6.5 0 0 0 3.9 1.3" />
            <path fill="#FBBC05" d="M7 7.7a6.5 6.5 0 0 0-3.9 11.7L6 16.5a2.5 2.5 0 1 1 3.3-3.3l2.9-2.9A6.5 6.5 0 0 0 7 7.7" />
        </svg>
    );
}

const PATHS: Record<Product, string> = {
    overview: "M4 4h7v7H4zm9 0h7v7h-7zM4 13h7v7H4zm9 0h7v7h-7z",
    run: "M12 2 3 7v10l9 5 9-5V7zm0 2.3 6.5 3.6L12 11.6 5.5 7.9zM5 9.6l6 3.4v6.6l-6-3.4zm8 10V13l6-3.4v6.6z",
    functions:
        "M9 4c-2 0-3 1.2-3 3v2.5C6 11 5 12 3.5 12 5 12 6 13 6 14.5V17c0 1.8 1 3 3 3v-2c-.7 0-1-.4-1-1v-2.6C8 13.2 7.4 12.4 6.6 12c.8-.4 1.4-1.2 1.4-2.4V7c0-.6.3-1 1-1zm6 0v2c.7 0 1 .4 1 1v2.6c0 1.2.6 2 1.4 2.4-.8.4-1.4 1.2-1.4 2.4V17c0 .6-.3 1-1 1v2c2 0 3-1.2 3-3v-2.5c0-1.5 1-2.5 2.5-2.5-1.5 0-2.5-1-2.5-2.5V7c0-1.8-1-3-3-3",
    compute: "M3 5h18v10H3zm2 2v6h14V7zM8 17h8v2H8z",
    gke: "M12 2 4 6.5v11L12 22l8-4.5v-11zm0 2.3 5.9 3.3v6.8L12 17.7l-5.9-3.3V7.6zM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8",
    storage: "M4 5h16v4H4zm0 5h16v4H4zm0 5h16v4H4zm2-8.5v1h2v-1zm0 5v1h2v-1zm0 5v1h2v-1z",
    sql: "M12 3C7.6 3 4 4.3 4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6c0-1.7-3.6-3-8-3m0 2c3.9 0 6 1.1 6 1s-2.1 1-6 1-6-1-6-1 2.1-1 6-1m6 13c0 .1-2.1 1-6 1s-6-.9-6-1v-2.3c1.5.8 3.6 1.3 6 1.3s4.5-.5 6-1.3zm0-5c0 .1-2.1 1-6 1s-6-.9-6-1v-2.3c1.5.8 3.6 1.3 6 1.3s4.5-.5 6-1.3z",
    pubsub: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6M5 3a2 2 0 1 0 0 4 2 2 0 0 0 0-4m14 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4M5 17a2 2 0 1 0 0 4 2 2 0 0 0 0-4m14 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4M6.4 6.4l3.2 3.2m4.8 0 3.2-3.2m-11.2 11.2 3.2-3.2m4.8 0 3.2 3.2",
    build: "M22 19 13 10a6 6 0 0 0-7.7-7.6l3.9 3.9-2.8 2.8-4-3.9A6 6 0 0 0 10 13l9 9z",
    secrets:
        "M12 2a5 5 0 0 0-5 5v3H5v12h14V10h-2V7a5 5 0 0 0-5-5m0 2a3 3 0 0 1 3 3v3H9V7a3 3 0 0 1 3-3m0 9a2 2 0 0 1 1 3.7V19h-2v-2.3A2 2 0 0 1 12 13",
    logging: "M4 5h16v2H4zm0 4h10v2H4zm0 4h16v2H4zm0 4h10v2H4z",
    bigquery:
        "M10.5 3a7.5 7.5 0 1 0 4.6 13.4l4.5 4.5 1.4-1.4-4.5-4.5A7.5 7.5 0 0 0 10.5 3m0 2a5.5 5.5 0 1 1 0 11 5.5 5.5 0 0 1 0-11M8 10h1.5v4H8zm2.3-2h1.5v6h-1.5zm2.2 3H14v3h-1.5z",
    registry: "M4 7 12 3l8 4v10l-8 4-8-4zm2 1.2v7.6l6 3 6-3V8.2l-6 3zm6 1.1 5.2-2.6L12 4.1 6.8 6.7z",
    scheduler: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m0 2a8 8 0 1 1 0 16 8 8 0 0 1 0-16m-1 3v6l5 3 1-1.6-4-2.4V7z",
    iam: "M12 2 4 5v6c0 5 3.4 9.7 8 11 4.6-1.3 8-6 8-11V5zm0 4a3 3 0 1 1 0 6 3 3 0 0 1 0-6m0 8c2 0 4 1 4.6 2.4A7 7 0 0 1 12 19.4a7 7 0 0 1-4.6-3C8 15 10 14 12 14",
    errors: "M12 2 1 21h22zm0 4 7.5 13h-15zm-1 4v4h2v-4zm0 6v2h2v-2z",
    network:
        "M12 2a3 3 0 0 0-1 5.8V11H6a2 2 0 0 0-2 2v3.2a3 3 0 1 0 2 0V13h5v3.2a3 3 0 1 0 2 0V13h5v3.2a3 3 0 1 0 2 0V13a2 2 0 0 0-2-2h-5V7.8A3 3 0 0 0 12 2",
    dns: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20m6.9 6h-3a15.6 15.6 0 0 0-1.4-3.6A8 8 0 0 1 18.9 8M12 4c.8 1.2 1.5 2.5 1.9 4h-3.8c.4-1.5 1.1-2.8 1.9-4M4.3 14a8 8 0 0 1 0-4h3.4a16.5 16.5 0 0 0 0 4zm.8 2h3a15.6 15.6 0 0 0 1.4 3.6A8 8 0 0 1 5.1 16M8 8H5.1a8 8 0 0 1 4.4-3.6A15.6 15.6 0 0 0 8 8m4 12c-.8-1.2-1.5-2.5-1.9-4h3.8c-.4 1.5-1.1 2.8-1.9 4m2.3-6H9.7a14.7 14.7 0 0 1 0-4h4.6a14.7 14.7 0 0 1 0 4m.3 5.6c.6-1.1 1.1-2.3 1.4-3.6h3a8 8 0 0 1-4.4 3.6m1.7-5.6a16.5 16.5 0 0 0 0-4h3.4a8 8 0 0 1 0 4z",
    monitoring: "M3 3h2v16h16v2H3zm4 12 4-5 3 3 5-7 1.6 1.2L14.3 16 11 12.7 8.6 15.8z",
};

export function ProductIcon({ product, size = 18 }: { product: Product; size?: number }) {
    const stroked = product === "pubsub";
    return (
        <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
            <path
                d={PATHS[product]}
                fill={stroked ? "currentColor" : "currentColor"}
                stroke={stroked ? "currentColor" : "none"}
                strokeWidth={stroked ? 1.2 : 0}
                fillRule="evenodd"
            />
        </svg>
    );
}
