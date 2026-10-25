import { invokeCommand } from "./host";

export interface PageRef {
  id: string;
  title: string;
  /** The page's height at the reply width, when the agent measured it. */
  height?: number;
}

/* An agent's pages live on its host, so the page can neither show them nor theme them. */
export const pagesApi = {
  theme: (_dark: boolean, _variables: Record<string, string>): Promise<void> =>
    invokeCommand("pages_theme"),
};
