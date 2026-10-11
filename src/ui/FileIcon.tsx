import { useSyncExternalStore } from "react";
import { languageOf } from "../languages";
import { swallow } from "../state/toast";
import { currentTheme, subscribeTheme } from "../themes/bus";

/** The Material file icon theme's lookup tables, as scripts/generate-file-icons.mjs writes them. */
export interface FileIconIndex {
    file: string;
    names: Record<string, string>;
    extensions: Record<string, string>;
    languages: Record<string, string>;
    light: Record<string, string>;
}

interface FileIconTables {
    file: string;
    names: Map<string, string>;
    extensions: Map<string, string>;
    languages: Map<string, string>;
    light: Map<string, string>;
}

function byName(grouped: Record<string, string>): Map<string, string> {
    const map = new Map<string, string>();
    for (const [icon, names] of Object.entries(grouped)) for (const name of names.split(" ")) map.set(name, icon);
    return map;
}

export function fileIconTables(index: FileIconIndex): FileIconTables {
    return {
        file: index.file,
        names: byName(index.names),
        extensions: byName(index.extensions),
        languages: new Map(Object.entries(index.languages)),
        light: new Map(Object.entries(index.light)),
    };
}

/** The icon a file shows, matched the way VS Code matches it: whole name, then longest extension, then language. */
export function fileIconOf(tables: FileIconTables, name: string): string {
    const lower = name.toLowerCase();
    const named = tables.names.get(lower);
    if (named) return named;
    for (let dot = lower.indexOf(".", 1); dot !== -1; dot = lower.indexOf(".", dot + 1)) {
        const icon = tables.extensions.get(lower.slice(dot + 1));
        if (icon) return icon;
    }
    const language = languageOf(name);
    return (language && tables.languages.get(language)) || tables.file;
}

let tables: FileIconTables | null = null;
const tableListeners = new Set<() => void>();

void fetch("/file-icons/index.json")
    .then((response) => response.json() as Promise<FileIconIndex>)
    .then((index) => {
        tables = fileIconTables(index);
        for (const listener of tableListeners) listener();
    })
    .catch(swallow("file icons"));

function subscribeTables(listener: () => void) {
    tableListeners.add(listener);
    return () => tableListeners.delete(listener);
}

const loadedTables = () => tables;
const isDark = () => currentTheme().dark;

export function ThemeIcon({ icon, size = 15 }: { icon: string | null; size?: number }) {
    const loaded = useSyncExternalStore(subscribeTables, loadedTables);
    const dark = useSyncExternalStore(subscribeTheme, isDark);
    if (!icon) return <span className="theme-icon" style={{ width: size, height: size }} />;
    const shown = (!dark && loaded?.light.get(icon)) || icon;
    return <img className="theme-icon" src={`/file-icons/${shown}.svg`} width={size} height={size} alt="" draggable={false} />;
}

export function FileIcon({ name, size = 14 }: { name: string; size?: number }) {
    const loaded = useSyncExternalStore(subscribeTables, loadedTables);
    return (
        <span className="file-glyph" aria-hidden="true">
            <ThemeIcon icon={loaded && fileIconOf(loaded, name)} size={size} />
        </span>
    );
}

interface DocumentKind {
    label: string;
    color: string;
}

const PDF: DocumentKind = { label: "PDF", color: "#e5252a" };
const WORD: DocumentKind = { label: "DOC", color: "#2b579a" };
const SHEET: DocumentKind = { label: "XLS", color: "#1d6f42" };
const SLIDES: DocumentKind = { label: "PPT", color: "#d24726" };
const ARCHIVE: DocumentKind = { label: "ZIP", color: "#7a7486" };

const DOCUMENTS: Record<string, DocumentKind> = {
    pdf: PDF,
    doc: WORD,
    docx: WORD,
    rtf: { label: "RTF", color: "#2b579a" },
    odt: WORD,
    pages: { label: "PAGES", color: "#f7a325" },
    xls: SHEET,
    xlsx: SHEET,
    xlsm: SHEET,
    ods: SHEET,
    numbers: { label: "NUM", color: "#1fa34a" },
    csv: { label: "CSV", color: "#3a9b5c" },
    tsv: { label: "TSV", color: "#3a9b5c" },
    ppt: SLIDES,
    pptx: SLIDES,
    odp: SLIDES,
    key: { label: "KEY", color: "#1f8cff" },
    zip: ARCHIVE,
    tar: { label: "TAR", color: "#7a7486" },
    gz: { label: "GZ", color: "#7a7486" },
    tgz: { label: "TGZ", color: "#7a7486" },
    "7z": { label: "7Z", color: "#7a7486" },
    rar: { label: "RAR", color: "#7a7486" },
    mp4: { label: "MP4", color: "#8e44ad" },
    mov: { label: "MOV", color: "#8e44ad" },
    webm: { label: "WEBM", color: "#8e44ad" },
    mp3: { label: "MP3", color: "#c0392b" },
    wav: { label: "WAV", color: "#c0392b" },
    m4a: { label: "M4A", color: "#c0392b" },
    txt: { label: "TXT", color: "#6d6878" },
};

function documentKind(name: string): DocumentKind | undefined {
    const i = name.lastIndexOf(".");
    return i > 0 ? DOCUMENTS[name.slice(i + 1).toLowerCase()] : undefined;
}

/**
 * A file drawn as a sheet of paper with its type on a coloured band. Office
 * files, PDFs, archives and media get one; anything else keeps the glyph the
 * file tree shows.
 */
export function FileTypeIcon({ name, size = 36 }: { name: string; size?: number }) {
    const kind = documentKind(name);
    if (!kind) return <FileIcon name={name} size={Math.round(size * 0.72)} />;
    return (
        <svg className="file-type-icon" width={(size * 30) / 36} height={size} viewBox="0 0 30 36" aria-hidden="true">
            <path d="M4 1h15l10 10v21a3 3 0 0 1-3 3H4a3 3 0 0 1-3-3V4a3 3 0 0 1 3-3z" fill="#e9e7ef" />
            <path d="M19 1v7a3 3 0 0 0 3 3h7" fill="#c9c5d4" />
            <rect x="0" y="17" rx="2.5" width="22" height="12" fill={kind.color} />
            <text
                x="11"
                y="25.6"
                textAnchor="middle"
                fill="#fff"
                fontFamily="-apple-system, BlinkMacSystemFont, system-ui, sans-serif"
                fontWeight="700"
                fontSize={kind.label.length > 3 ? 5.6 : 7.2}>
                {kind.label}
            </text>
        </svg>
    );
}
