interface ImportMetaEnv {
    readonly DEV: boolean;
    readonly VITE_TERMINAL_WEBGL?: string;
}

interface ImportMeta {
    readonly env: ImportMetaEnv;
}

declare module "*.css";

declare module "*.svg" {
    const url: string;
    export default url;
}

declare module "*?url" {
    const url: string;
    export default url;
}
