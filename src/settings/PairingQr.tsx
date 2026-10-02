import { useMemo } from "react";
import { encode } from "uqr";

/** Scanners need a light margin of four modules around the code. */
const QUIET_ZONE = 4;
/** Whole pixels per module, so every module draws the same size. */
const MODULE_PX = 3;

/** The pairing link as a QR code, dark on light so every phone camera reads it. */
export function PairingQr({ link }: { link: string }) {
    const path = useMemo(() => {
        const { data } = encode(link, { ecc: "M", border: 0 });
        let drawn = "";
        data.forEach((row, y) =>
            row.forEach((dark, x) => {
                if (dark) drawn += `M${x + QUIET_ZONE} ${y + QUIET_ZONE}h1v1h-1z`;
            }),
        );
        return { drawn, span: data.length + QUIET_ZONE * 2 };
    }, [link]);
    return (
        <svg
            className="pairing-qr"
            width={path.span * MODULE_PX}
            height={path.span * MODULE_PX}
            viewBox={`0 0 ${path.span} ${path.span}`}
            role="img"
            aria-label="Pairing QR code"
            shapeRendering="crispEdges">
            <rect width={path.span} height={path.span} fill="#fff" />
            <path d={path.drawn} fill="#000" />
        </svg>
    );
}
