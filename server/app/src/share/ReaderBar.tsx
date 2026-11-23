import type { ReactNode } from "react";

import sikemuxLockup from "../../../../brand/wordmark/lockup-white.svg";

export const GET_SIKEMUX =
  "https://sikemux.com/?utm_source=share&utm_medium=share&utm_campaign=public-link";

/** The bar over what someone shared: whose app it is, what is shared, and where to get Sikemux. */
export function ReaderBar({ children }: { children: ReactNode }) {
  return (
    <header className="shared-reader-top">
      <a href={GET_SIKEMUX} aria-label="Sikemux">
        <img src={sikemuxLockup} alt="Sikemux" />
      </a>
      {children}
      <a className="shared-reader-get" href={GET_SIKEMUX}>
        Get Sikemux
      </a>
    </header>
  );
}
