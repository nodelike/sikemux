import { createContext, useContext } from "react";

/**
 * A transcript from someone else's computer, such as a chat shared with this one. Its files
 * are not here: the pictures pasted into it come from its host, and nothing it names is looked
 * up or opened on this computer.
 */
export interface RemoteOrigin {
  /** A picture pasted into the chat, as a data URL, or null when the host will not send it. */
  readImage(path: string): Promise<string | null>;
}

export const RemoteOriginContext = createContext<RemoteOrigin | null>(null);

export function useRemoteOrigin(): RemoteOrigin | null {
  return useContext(RemoteOriginContext);
}
