import { useUser } from "@clerk/react";
import { initials } from "@mac/lib/initials";
import { useState } from "react";

/** The account's picture, or its initials on a neutral circle when it has none. */
export function Avatar({ size = 26 }: { size?: number }) {
  const { user } = useUser();
  const [failed, setFailed] = useState<string>();
  const picture = user?.hasImage ? user.imageUrl : undefined;
  const letters = user
    ? initials(user.fullName, user.primaryEmailAddress?.emailAddress)
    : "";
  return (
    <span
      className="avatar"
      aria-hidden="true"
      style={size === 26 ? undefined : { width: size, height: size }}
    >
      {picture && picture !== failed ? (
        <img
          src={`${picture}${picture.includes("?") ? "&" : "?"}width=${size * 2}&height=${size * 2}&fit=scale-down&quality=100`}
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
