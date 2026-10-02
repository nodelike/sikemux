import { invokeCommand as invoke } from "./invoke";

export interface DocumentPlacement {
    x: number;
    y: number;
    width: number;
    height: number;
}

export const documentPreviewApi = {
    show: (owner: string, path: string, placement: DocumentPlacement) => invoke<void>("document_preview_show", { owner, path, placement }),
    hide: (owner: string) => invoke<void>("document_preview_hide", { owner }),
};
