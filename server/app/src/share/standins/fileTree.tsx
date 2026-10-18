/* Stands in for the Mac's file tree in a browser: a shared chat's files have no menu here. */
export interface CtxItem {
  label?: string;
  sep?: boolean;
  run?: () => void;
}

export function TreeContextMenu(_props: { onClose: () => void }) {
  return null;
}
