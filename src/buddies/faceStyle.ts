import { coreCall } from "../api/core";
import type { FaceStyle } from "../api/generated/wire";
import { getIpcTransport } from "../api/transport";
import { setState } from "../state/store";
import { swallow } from "../state/toast";

const FACE_STYLE_EVENT = "face_style_changed";

/** Keeps how agents' faces look as the core keeps it, for as long as the page runs. */
export function watchFaceStyle(): () => void {
    const controller = new AbortController();
    void getIpcTransport()
        .subscribe<FaceStyle>(FACE_STYLE_EVENT, ({ payload }) => setState({ faceStyle: payload }), { signal: controller.signal })
        .catch(swallow("follow the face style"));
    coreCall<FaceStyle>("faceStyle")
        .then((faceStyle) => {
            if (!controller.signal.aborted) setState({ faceStyle });
        })
        .catch(swallow("read the face style"));
    return () => controller.abort();
}
