import type { ReactNode } from "react";

/* A shared chat's files have no menu on the page. */
export interface CtxItem {
  label?: string;
  icon?: ReactNode;
  sep?: boolean;
  run?: () => void;
}

export function TreeContextMenu(_props: {
  x: number;
  y: number;
  items: CtxItem[];
  onClose: () => void;
}) {
  return null;
}
