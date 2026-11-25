import type { ReactNode } from "react";

import sikemuxLockup from "../../../../brand/wordmark/lockup-purple.svg";

export const GET_SIKEMUX =
  "https://sikemux.com/?utm_source=share&utm_medium=share&utm_campaign=public-link";

/** The bar over what someone shared: whose app it is, what is shared, and where to get Sikemux. */
export function ReaderBar({
  badge,
  title,
}: {
  badge?: ReactNode;
  title?: string;
}) {
  return (
    <header className="sh-bar">
      <a className="sh-brand" href={GET_SIKEMUX} aria-label="Sikemux">
        <img src={sikemuxLockup} alt="" />
      </a>
      {badge}
      <span className="sh-gap" />
      {title ? (
        <>
          <span className="sh-bar-title" title={title}>
            {title}
          </span>
          <span className="sh-gap" />
        </>
      ) : null}
      <a className="sh-btn" href={GET_SIKEMUX}>
        Get Sikemux
      </a>
    </header>
  );
}
