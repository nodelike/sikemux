// The .bru model as the plugin reads and writes it.

import type { BruHttpMethod } from "../types.generated";

export type {
    BruAuth,
    BruAuthMode as AuthMode,
    BruBody,
    BruBodyMode as BodyMode,
    BruCollection,
    BruEnv,
    BruHttpMethod as HttpMethod,
    BruKeyVal as KeyVal,
    BruMeta,
    BruRawBlock as RawBlock,
    BruRequest,
    BruRequestView,
    BruScope,
    BruTreeNode,
} from "../types.generated";

export const HTTP_METHODS: BruHttpMethod[] = ["get", "post", "put", "delete", "patch", "options", "head"];
