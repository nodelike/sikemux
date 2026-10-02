import { languageOf } from "../languages";

interface GlyphInfo {
    char: string;
    color: string;
}

const DEFAULT: GlyphInfo = { char: "", color: "var(--ink-faint)" };

const SPECIAL: Record<string, GlyphInfo> = {
    "package.json": { char: "", color: "#cb3837" },
    "package-lock.json": { char: "", color: "#cbcb41" },
    "pnpm-lock.yaml": { char: "", color: "#f9ad00" },
    "yarn.lock": { char: "", color: "#2c8ebb" },
    dockerfile: { char: "", color: "#2496ed" },
    ".dockerignore": { char: "", color: "#2496ed" },
    makefile: { char: "", color: "#a4aa7b" },
    ".gitignore": { char: "", color: "#f05033" },
    ".gitattributes": { char: "", color: "#f05033" },
    ".gitmodules": { char: "", color: "#f05033" },
    "readme.md": { char: "", color: "#519aba" },
    readme: { char: "", color: "#519aba" },
    license: { char: "", color: "#cbcb41" },
    "tsconfig.json": { char: "", color: "#3178c6" },
    "tsconfig.node.json": { char: "", color: "#3178c6" },
    "vite.config.ts": { char: "", color: "#646cff" },
    "vite.config.js": { char: "", color: "#646cff" },
    "cargo.toml": { char: "", color: "#dea584" },
    "cargo.lock": { char: "", color: "#dea584" },
    "tauri.conf.json": { char: "", color: "#ffc131" },
    ".prettierrc": { char: "", color: "#c596c7" },
    ".eslintrc": { char: "", color: "#4b32c3" },
};

const BY_EXT: Record<string, GlyphInfo> = {
    ts: { char: "", color: "#3178c6" },
    tsx: { char: "", color: "#3178c6" },
    js: { char: "", color: "#f7df1e" },
    jsx: { char: "", color: "#f7df1e" },
    mjs: { char: "", color: "#f7df1e" },
    cjs: { char: "", color: "#f7df1e" },
    json: { char: "", color: "#fbbf24" },
    jsonc: { char: "", color: "#fbbf24" },
    html: { char: "", color: "#e34f26" },
    htm: { char: "", color: "#e34f26" },
    css: { char: "", color: "#1572b6" },
    scss: { char: "", color: "#cf649a" },
    sass: { char: "", color: "#cf649a" },
    less: { char: "", color: "#1d365d" },
    md: { char: "", color: "#519aba" },
    mdx: { char: "", color: "#519aba" },
    yaml: { char: "", color: "#cbcb41" },
    yml: { char: "", color: "#cbcb41" },
    toml: { char: "", color: "#9c4221" },
    rs: { char: "", color: "#dea584" },
    go: { char: "", color: "#00add8" },
    py: { char: "", color: "#3572a5" },
    pyc: { char: "", color: "#3572a5" },
    rb: { char: "", color: "#cc342d" },
    lua: { char: "", color: "#7884e7" },
    java: { char: "", color: "#ea2d2e" },
    kt: { char: "", color: "#f18e33" },
    swift: { char: "", color: "#f05138" },
    c: { char: "", color: "#599eff" },
    cpp: { char: "", color: "#9c033a" },
    cc: { char: "", color: "#9c033a" },
    h: { char: "", color: "#a074c4" },
    hpp: { char: "", color: "#a074c4" },
    sh: { char: "", color: "#89e051" },
    bash: { char: "", color: "#89e051" },
    zsh: { char: "", color: "#89e051" },
    fish: { char: "", color: "#89e051" },
    vim: { char: "", color: "#019733" },
    sql: { char: "", color: "#dad8d8" },
    tf: { char: "", color: "#7b42bc" },
    hcl: { char: "", color: "#7b42bc" },
    env: { char: "", color: "#ecd53f" },
    txt: { char: "", color: "#888" },
    log: { char: "", color: "#888" },
    pdf: { char: "", color: "#e94e4e" },
    png: { char: "", color: "#a074c4" },
    jpg: { char: "", color: "#a074c4" },
    jpeg: { char: "", color: "#a074c4" },
    gif: { char: "", color: "#a074c4" },
    webp: { char: "", color: "#a074c4" },
    bmp: { char: "", color: "#a074c4" },
    avif: { char: "", color: "#a074c4" },
    tif: { char: "", color: "#a074c4" },
    tiff: { char: "", color: "#a074c4" },
    ico: { char: "", color: "#a074c4" },
    svg: { char: "", color: "#ffb13b" },
    zip: { char: "", color: "#888" },
    tar: { char: "", color: "#888" },
    gz: { char: "", color: "#888" },
    woff: { char: "", color: "#888" },
    woff2: { char: "", color: "#888" },
    ttf: { char: "", color: "#888" },
    otf: { char: "", color: "#888" },
};

/** Every other file a grammar reads takes its language's glyph. */
const BY_LANGUAGE: Record<string, GlyphInfo> = {
    ada: { char: "", color: "#599eff" },
    apl: { char: "", color: "#24a148" },
    applescript: { char: "", color: "#6d8085" },
    asm: { char: "", color: "#0091bd" },
    astro: { char: "", color: "#e23f67" },
    awk: { char: "", color: "#4d5a5e" },
    bat: { char: "", color: "#c1f12e" },
    bibtex: { char: "󱉟", color: "#cbcb41" },
    bicep: { char: "", color: "#519aba" },
    c: { char: "", color: "#599eff" },
    clojure: { char: "", color: "#8dc149" },
    cmake: { char: "", color: "#dce3eb" },
    cobol: { char: "", color: "#005ca5" },
    coffee: { char: "", color: "#cbcb41" },
    cpp: { char: "", color: "#9c033a" },
    crystal: { char: "", color: "#c8c8c8" },
    csharp: { char: "󰌛", color: "#596706" },
    css: { char: "", color: "#1572b6" },
    csv: { char: "", color: "#89e051" },
    cue: { char: "󰲹", color: "#ed95ae" },
    d: { char: "", color: "#b03931" },
    dart: { char: "", color: "#03589c" },
    desktop: { char: "", color: "#563d7c" },
    diff: { char: "", color: "#41535b" },
    docker: { char: "󰡨", color: "#458ee6" },
    dotenv: { char: "", color: "#ecd53f" },
    elixir: { char: "", color: "#b88cd9" },
    elm: { char: "", color: "#519aba" },
    "emacs-lisp": { char: "", color: "#8172be" },
    erb: { char: "", color: "#701516" },
    erlang: { char: "", color: "#b83998" },
    fish: { char: "", color: "#89e051" },
    "fortran-free-form": { char: "󱈚", color: "#734f96" },
    fsharp: { char: "", color: "#519aba" },
    gdresource: { char: "", color: "#6d8086" },
    gdscript: { char: "", color: "#6d8086" },
    gherkin: { char: "", color: "#00a818" },
    gleam: { char: "", color: "#ffaff3" },
    glsl: { char: "", color: "#5586a6" },
    go: { char: "", color: "#00add8" },
    graphql: { char: "", color: "#e535ab" },
    groovy: { char: "", color: "#005f87" },
    haml: { char: "", color: "#eaeae1" },
    handlebars: { char: "", color: "#f0772b" },
    haskell: { char: "", color: "#a074c4" },
    haxe: { char: "", color: "#ea8220" },
    hcl: { char: "", color: "#7b42bc" },
    html: { char: "", color: "#e34f26" },
    http: { char: "", color: "#008ec7" },
    hurl: { char: "", color: "#ff0288" },
    ini: { char: "", color: "#6d8086" },
    java: { char: "", color: "#ea2d2e" },
    javascript: { char: "", color: "#f7df1e" },
    json: { char: "", color: "#fbbf24" },
    json5: { char: "", color: "#cbcb41" },
    jsonc: { char: "", color: "#fbbf24" },
    jsonl: { char: "", color: "#cbcb41" },
    jsx: { char: "", color: "#f7df1e" },
    julia: { char: "", color: "#a270ba" },
    just: { char: "", color: "#6d8086" },
    kotlin: { char: "", color: "#f18e33" },
    latex: { char: "", color: "#3d6117" },
    less: { char: "", color: "#1d365d" },
    liquid: { char: "", color: "#95bf47" },
    log: { char: "", color: "#888" },
    lua: { char: "", color: "#7884e7" },
    luau: { char: "", color: "#00a2ff" },
    make: { char: "", color: "#6d8086" },
    markdown: { char: "", color: "#519aba" },
    mdx: { char: "", color: "#519aba" },
    mojo: { char: "", color: "#ff4c1f" },
    nim: { char: "", color: "#f3d400" },
    nix: { char: "", color: "#7ebae4" },
    nushell: { char: "", color: "#3aa675" },
    "objective-c": { char: "", color: "#599eff" },
    "objective-cpp": { char: "", color: "#519aba" },
    ocaml: { char: "", color: "#e37933" },
    odin: { char: "󰟢", color: "#3882d2" },
    openscad: { char: "", color: "#f9d72c" },
    org: { char: "", color: "#77aa99" },
    perl: { char: "", color: "#519aba" },
    php: { char: "", color: "#f05340" },
    plsql: { char: "", color: "#6d8086" },
    po: { char: "", color: "#2596be" },
    powershell: { char: "󰨊", color: "#4273ca" },
    prisma: { char: "", color: "#5a67d8" },
    prolog: { char: "", color: "#e4b854" },
    puppet: { char: "", color: "#ffa61a" },
    python: { char: "", color: "#3572a5" },
    qml: { char: "", color: "#40cd52" },
    qss: { char: "", color: "#40cd52" },
    r: { char: "󰟔", color: "#2266ba" },
    racket: { char: "󰘧", color: "#9f1d20" },
    razor: { char: "󱦗", color: "#512bd4" },
    ruby: { char: "", color: "#cc342d" },
    rust: { char: "", color: "#dea584" },
    sass: { char: "", color: "#cf649a" },
    scala: { char: "", color: "#cc3e44" },
    scheme: { char: "󰘧", color: "#eeeeee" },
    scss: { char: "", color: "#cf649a" },
    shellscript: { char: "", color: "#89e051" },
    shellsession: { char: "", color: "#89e051" },
    solidity: { char: "", color: "#519aba" },
    sql: { char: "", color: "#dad8d8" },
    stylus: { char: "", color: "#8dc149" },
    svelte: { char: "", color: "#ff4785" },
    swift: { char: "", color: "#f05138" },
    "system-verilog": { char: "󰍛", color: "#019833" },
    tcl: { char: "󰛓", color: "#1e5cb3" },
    templ: { char: "", color: "#dbbd30" },
    terraform: { char: "", color: "#7b42bc" },
    toml: { char: "", color: "#9c4221" },
    tsx: { char: "", color: "#3178c6" },
    twig: { char: "", color: "#8dc149" },
    typescript: { char: "", color: "#3178c6" },
    typst: { char: "", color: "#0dbcc0" },
    v: { char: "󰍛", color: "#019833" },
    vala: { char: "", color: "#7b3db9" },
    verilog: { char: "󰍛", color: "#019833" },
    vhdl: { char: "󰍛", color: "#019833" },
    viml: { char: "", color: "#019733" },
    vue: { char: "", color: "#ff4785" },
    wasm: { char: "", color: "#5c4cdb" },
    xml: { char: "", color: "#ffb13b" },
    xsl: { char: "󰗀", color: "#33a9dc" },
    yaml: { char: "", color: "#cbcb41" },
    zig: { char: "", color: "#f69a1b" },
};

function lookup(name: string): GlyphInfo {
    const lower = name.toLowerCase();
    if (SPECIAL[lower]) return SPECIAL[lower];
    if (lower.startsWith(".env")) return { char: "", color: "#ecd53f" };
    const i = name.lastIndexOf(".");
    const byExtension = i > 0 ? BY_EXT[name.slice(i + 1).toLowerCase()] : undefined;
    if (byExtension) return byExtension;
    const language = languageOf(name);
    return (language && BY_LANGUAGE[language]) || DEFAULT;
}

export function FileIcon({ name, size = 15 }: { name: string; size?: number }) {
    const { char, color } = lookup(name);
    return (
        <span className="file-glyph" style={{ color, fontSize: size }} aria-hidden="true">
            {char}
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
