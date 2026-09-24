// Data model for the Bruno (.bru) client.
//
// A .bru file is a small block-based DSL. We parse it into the structured
// model below, while preserving any block we don't explicitly model (e.g.
// `example`) verbatim in `extra` so save-back round-trips faithfully.

export type HttpMethod = "get" | "post" | "put" | "delete" | "patch" | "options" | "head";

export const HTTP_METHODS: HttpMethod[] = ["get", "post", "put", "delete", "patch", "options", "head"];

/** A key/value row (headers, query params, form fields, vars). `~`-prefixed keys are disabled. */
export interface KeyVal {
    name: string;
    value: string;
    enabled: boolean;
}

export type BodyMode = "none" | "json" | "text" | "xml" | "sparql" | "graphql" | "form-urlencoded" | "multipart-form" | "file";

export interface BruBody {
    mode: BodyMode;
    /** raw text for json / text / xml / sparql / graphql */
    text: string;
    /** graphql query + variables (json text) */
    graphql: { query: string; variables: string };
    /** rows for form-urlencoded / multipart-form (multipart values may be `@file(/path)`) */
    form: KeyVal[];
}

export type AuthMode = "none" | "inherit" | "bearer" | "basic" | "apikey" | "awsv4" | "digest" | "wsse" | "oauth2";

export interface BruAuth {
    mode: AuthMode;
    bearer: { token: string };
    basic: { username: string; password: string };
    apikey: { key: string; value: string; placement: "header" | "queryparams" };
}

export interface BruMeta {
    name: string;
    type: string;
    seq?: number;
}

export interface BruSettings {
    encodeUrl?: boolean;
    timeout?: number;
}

/** A block we parse generically but don't fold into the typed model — kept for round-trip. */
export interface RawBlock {
    name: string;
    delim: "{" | "[";
    raw: string;
}

/** A parsed .bru request file. */
export interface BruRequest {
    meta: BruMeta;
    method: HttpMethod;
    url: string;
    params: { query: KeyVal[]; path: KeyVal[] };
    headers: KeyVal[];
    body: BruBody;
    auth: BruAuth;
    vars: { req: KeyVal[]; res: KeyVal[] };
    scripts: { pre: string; post: string };
    assertions: KeyVal[];
    tests: string;
    docs: string;
    settings: BruSettings;
    extra: RawBlock[];
}

/** Shared collection-level / folder-level config (collection.bru, folder.bru). */
export interface BruScope {
    meta: BruMeta;
    auth: BruAuth;
    headers: KeyVal[];
    vars: { req: KeyVal[]; res: KeyVal[] };
    scripts: { pre: string; post: string };
    extra: RawBlock[];
}

/** An environment file under <collection>/environments/<name>.bru */
export interface BruEnv {
    /** stable unique key: `${collectionPath}::${name}` */
    id: string;
    name: string;
    /** the collection root this environment belongs to */
    collectionPath: string;
    collectionName: string;
    vars: KeyVal[];
    secretNames: string[];
}

export interface BruTreeRequest {
    type: "request";
    name: string;
    path: string;
    seq: number;
    method: HttpMethod;
    /** the collection root this request belongs to (for scoping environments) */
    collectionPath: string;
    request: BruRequest;
}

export interface BruTreeFolder {
    type: "folder";
    name: string;
    path: string;
    seq: number;
    scope: BruScope | null;
    children: BruTreeNode[];
}

export type BruTreeNode = BruTreeRequest | BruTreeFolder;

export interface BruCollection {
    rootPath: string;
    name: string;
    config: BruScope | null;
    envs: BruEnv[];
    tree: BruTreeNode[];
}

export function emptyBody(): BruBody {
    return { mode: "none", text: "", graphql: { query: "", variables: "" }, form: [] };
}

export function emptyAuth(mode: AuthMode = "none"): BruAuth {
    return {
        mode,
        bearer: { token: "" },
        basic: { username: "", password: "" },
        apikey: { key: "", value: "", placement: "header" },
    };
}

export function emptyRequest(name = "request"): BruRequest {
    return {
        meta: { name, type: "http" },
        method: "get",
        url: "",
        params: { query: [], path: [] },
        headers: [],
        body: emptyBody(),
        auth: emptyAuth("inherit"),
        vars: { req: [], res: [] },
        scripts: { pre: "", post: "" },
        assertions: [],
        tests: "",
        docs: "",
        settings: { encodeUrl: true, timeout: 0 },
        extra: [],
    };
}

export function emptyScope(name: string): BruScope {
    return {
        meta: { name, type: "" },
        auth: emptyAuth("inherit"),
        headers: [],
        vars: { req: [], res: [] },
        scripts: { pre: "", post: "" },
        extra: [],
    };
}
