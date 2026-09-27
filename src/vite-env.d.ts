interface ImportMetaEnv {
    readonly DEV: boolean;
    readonly VITE_TERMINAL_WEBGL?: string;
}

interface ImportMeta {
    readonly env: ImportMetaEnv;
}

declare module "*.css";
