export interface FolderLanguage {
    readonly id: "java" | "node" | "go" | "rust" | "python";
    readonly markers: readonly string[];
    /** The version the folder's files ask for, such as `17`. */
    readonly version?: string;
    readonly framework?: "spring-boot";
}

export interface FolderEnvFile {
    /** Relative to the folder. */
    readonly path: string;
    /** Null when every environment reads it. */
    readonly environment: string | null;
    readonly local: boolean;
    readonly template: boolean;
    readonly loadedBy: readonly string[];
    readonly variables: readonly { key: string; value: string }[];
}

export interface ProjectFolder {
    /** Relative to the project, `.` for its root. */
    readonly path: string;
    readonly languages: readonly FolderLanguage[];
    readonly envFiles: readonly FolderEnvFile[];
    readonly environments: readonly { name: string; production: boolean }[];
}

export interface FolderSettings {
    /** Absent loads no files; empty loads only those every environment shares. */
    readonly environment?: string;
    readonly env?: Readonly<Record<string, string>>;
    /** The tasks an agent's work in the folder must pass before it is done, by id. */
    readonly done?: readonly string[];
    /** Whose env files agents get when not the person's; production never reaches them. */
    readonly agentEnvironment?: string;
    /** docker-compose services the folder's tasks need running first. */
    readonly services?: readonly string[];
}

/** What the person tells every agent working in the project. */
export interface ProjectRules {
    readonly notes?: string;
    readonly checkEveryFolder?: boolean;
    readonly stopOnFailure?: boolean;
    /** Path globs agents must not change without asking. */
    readonly neverEdit?: readonly string[];
    /** Command patterns an agent asks before running. */
    readonly askBefore?: readonly string[];
    /** Command patterns agents never run. */
    readonly neverRun?: readonly string[];
}

/** One setting a language's page offers, as the core lists them; each sets one variable. */
export interface LanguageField {
    readonly section: string;
    readonly label: string;
    readonly desc: string;
    readonly kind: "jdk" | "text" | "toggle" | "choice" | "version";
    /** Empty for a `version` field, which only shows what the files ask for. */
    readonly variable: string;
    readonly placeholder?: string;
    /** Written before what the person types, such as `-P`. */
    readonly prefix?: string;
    readonly options?: readonly string[];
    readonly when?: "maven" | "springBoot";
}

export interface ProjectFolders {
    readonly folders: readonly ProjectFolder[];
    /** Each language's fields, by language id. */
    readonly fields: Readonly<Record<string, readonly LanguageField[]>>;
    readonly settings: Readonly<Record<string, FolderSettings>>;
    readonly rules: ProjectRules;
}

export const LANGUAGE_NAMES: Record<FolderLanguage["id"], string> = {
    java: "Java",
    node: "Node",
    go: "Go",
    rust: "Rust",
    python: "Python",
};

export function folderName(folder: string, projectName: string): string {
    return folder === "." ? projectName : `${folder}/`;
}

const SECRET = /key|secret|token|passw|pwd|private|credential|auth|cookie|session|dsn/i;

/** A value that should stay hidden until asked for, by what its variable is called. */
export function looksSecret(key: string): boolean {
    return SECRET.test(key);
}
