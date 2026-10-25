import { useEffect, useState } from "react";
import { IconGlobe } from "@shared/ui/Icons";

/** The site's own mark once it has arrived, and a globe until then. */
export function SiteIcon({ src }: { src: string | null }) {
    const [broken, setBroken] = useState(false);
    useEffect(() => setBroken(false), [src]);
    if (!src || broken) return <IconGlobe size={13} />;
    return <img className="tab-favicon" src={src} alt="" onError={() => setBroken(true)} />;
}
