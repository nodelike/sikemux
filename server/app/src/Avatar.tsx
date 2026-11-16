import { clerkImageUrl } from "@shared/lib/format";
import { useUser } from "@clerk/react";
import { initials } from "@shared/lib/initials";
import { useState } from "react";

/** The account's picture, or its initials on a neutral circle when it has none. */
export function Avatar({ size = 26 }: { size?: number }) {
  const { user } = useUser();
  return (
    <Face
      size={size}
      picture={user?.hasImage ? user.imageUrl : undefined}
      letters={
        user
          ? initials(user.fullName, user.primaryEmailAddress?.emailAddress)
          : ""
      }
    />
  );
}

/** Someone else's picture, or their initials. */
export function PersonAvatar({
  name,
  imageUrl,
  size = 26,
}: {
  name: string | null;
  imageUrl: string | null;
  size?: number;
}) {
  return (
    <Face
      size={size}
      picture={imageUrl ?? undefined}
      letters={initials(name, undefined)}
    />
  );
}

function Face({
  size,
  picture,
  letters,
}: {
  size: number;
  picture: string | undefined;
  letters: string;
}) {
  const [failed, setFailed] = useState<string>();
  return (
    <span
      className="avatar"
      aria-hidden="true"
      style={size === 26 ? undefined : { width: size, height: size }}
    >
      {picture && picture !== failed ? (
        <img
          src={clerkImageUrl(picture, size * 2)}
          alt=""
          width={size}
          height={size}
          onError={() => setFailed(picture)}
        />
      ) : (
        letters
      )}
    </span>
  );
}
