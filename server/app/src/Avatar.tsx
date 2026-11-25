import { useUser } from "@clerk/react";
import type { Org } from "@sikemux/protocol";
import { avatarSeed } from "@sikemux/protocol/avatar";
import { clerkImageUrl } from "@shared/lib/format";
import { useId, useMemo, useState, type CSSProperties } from "react";

import { memberSvg, orgSvg } from "./avatarSvg.ts";
import { logoFailed, useLogo } from "./logos.ts";

const sized = (size: number | undefined) =>
  size ? ({ "--z": `${size}px` } as CSSProperties) : undefined;

/** The signed-in person's own picture, or one drawn from their account's id. */
export function Avatar({ size }: { size?: number }) {
  const { user } = useUser();
  return (
    <PersonAvatar
      seed={avatarSeed(user?.id ?? "")}
      imageUrl={user?.hasImage ? user.imageUrl : null}
      size={size}
    />
  );
}

export function PersonAvatar({
  seed,
  imageUrl,
  size,
}: {
  seed: string;
  imageUrl: string | null;
  size?: number;
}) {
  const [failed, setFailed] = useState<string>();
  const key = useId();
  const svg = useMemo(() => memberSvg(seed, key), [seed, key]);
  if (imageUrl && imageUrl !== failed)
    return (
      <span className="av" aria-hidden="true" style={sized(size)}>
        <img
          src={clerkImageUrl(imageUrl, (size ?? 64) * 2)}
          alt=""
          onError={() => setFailed(imageUrl)}
        />
      </span>
    );
  return (
    <span
      className="av"
      aria-hidden="true"
      style={sized(size)}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

/** An org's logo, or a picture drawn from its id when it has none. */
export function OrgAvatar({
  org,
  size,
}: {
  org: Pick<Org, "id" | "logo">;
  size?: number;
}) {
  const key = useId();
  const svg = useMemo(() => orgSvg(org.id, key), [org.id, key]);
  const logo = useLogo(org);
  if (logo?.state === "ready")
    return (
      <span className="orgm" aria-hidden="true" style={sized(size)}>
        <img
          src={logo.url}
          alt=""
          onError={() => org.logo && logoFailed(org.logo)}
        />
      </span>
    );
  return (
    <span
      className="orgm"
      aria-hidden="true"
      style={sized(size)}
      dangerouslySetInnerHTML={{
        __html: logo?.state === "loading" ? "" : svg,
      }}
    />
  );
}
