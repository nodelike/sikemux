import { useUser } from "@clerk/react";
import { clerkImageUrl } from "@shared/lib/format";
import { useId, useMemo, useState, type CSSProperties } from "react";

import { orgSvg, personSvg } from "./avatarSvg.ts";

const sized = (size: number | undefined) =>
  size ? ({ "--z": `${size}px` } as CSSProperties) : undefined;

/** The signed-in person's own picture, or one drawn from their account's id. */
export function Avatar({ size }: { size?: number }) {
  const { user } = useUser();
  return (
    <PersonAvatar
      id={user?.id ?? ""}
      imageUrl={user?.hasImage ? user.imageUrl : null}
      size={size}
    />
  );
}

export function PersonAvatar({
  id,
  imageUrl,
  size,
}: {
  id: string;
  imageUrl: string | null;
  size?: number;
}) {
  const [failed, setFailed] = useState<string>();
  const key = useId();
  const svg = useMemo(() => personSvg(id, key), [id, key]);
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

export function OrgAvatar({ id, size }: { id: string; size?: number }) {
  const key = useId();
  const svg = useMemo(() => orgSvg(id, key), [id, key]);
  return (
    <span
      className="orgm"
      aria-hidden="true"
      style={sized(size)}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
