import { createContext } from "react";

/**
 * A shared chat comes from someone else's computer. Its files are not here: the pictures pasted
 * into it come from its host, and nothing it names is looked up or opened on this computer.
 */
export interface RemoteOrigin {
  /** A picture pasted into the chat, as a data URL, or null when the host will not send it. */
  readImage(path: string): Promise<string | null>;
}

export const RemoteOriginContext = createContext<RemoteOrigin>({
  readImage: () => Promise.resolve(null),
});
