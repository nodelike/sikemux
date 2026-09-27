import { GRAMMAR_ALIASES, GRAMMARS } from "./generated/grammars";

/** Files known by their whole name rather than by an extension. */
const FILE_NAMES: Readonly<Record<string, string>> = {
    ".editorconfig": "ini",
    ".env": "dotenv",
    "cmakelists.txt": "cmake",
    codeowners: "codeowners",
    "config.ru": "ruby",
    containerfile: "docker",
    gemfile: "ruby",
    gnumakefile: "make",
    rakefile: "ruby",
    ssh_config: "ssh-config",
};

/** Extensions that are neither a grammar's name nor one of its aliases. */
const EXTENSIONS: Readonly<Record<string, string>> = {
    bazel: "python",
    bib: "bibtex",
    bicepparam: "bicep",
    bzl: "python",
    "c++": "cpp",
    cbl: "cobol",
    cc: "cpp",
    ccm: "cpp",
    cfg: "ini",
    cljc: "clojure",
    cljd: "clojure",
    cljs: "clojure",
    cob: "cobol",
    cppm: "cpp",
    cr: "crystal",
    csh: "shellscript",
    cshtml: "razor",
    cson: "coffee",
    csproj: "xml",
    cxx: "cpp",
    cxxm: "cpp",
    ebuild: "shellscript",
    edn: "clojure",
    el: "emacs-lisp",
    env: "dotenv",
    epp: "puppet",
    ex: "elixir",
    exs: "elixir",
    "f#": "fsharp",
    feature: "gherkin",
    frag: "glsl",
    fsi: "fsharp",
    fsscript: "fsharp",
    fsx: "fsharp",
    gemspec: "ruby",
    geom: "glsl",
    gradle: "groovy",
    h: "c",
    hh: "cpp",
    hpp: "cpp",
    hrl: "erlang",
    htm: "html",
    hx: "haxe",
    hxx: "cpp",
    ino: "cpp",
    ipynb: "json",
    ixx: "cpp",
    ksh: "shellscript",
    lhs: "haskell",
    m: "objective-c",
    mk: "make",
    ml: "ocaml",
    mli: "ocaml",
    mm: "objective-cpp",
    mustache: "handlebars",
    patch: "diff",
    pck: "plsql",
    pl: "perl",
    plist: "xml",
    pls: "plsql",
    pm: "perl",
    pp: "puppet",
    pro: "prolog",
    pyi: "python",
    pyw: "python",
    rake: "ruby",
    rkt: "racket",
    rss: "xml",
    s: "asm",
    sbt: "scala",
    sc: "scala",
    scm: "scheme",
    sol: "solidity",
    sv: "system-verilog",
    svg: "xml",
    svh: "system-verilog",
    tex: "latex",
    ui: "xml",
    vert: "glsl",
    vh: "verilog",
    vhd: "vhdl",
    webmanifest: "json",
    xaml: "xml",
    xhtml: "html",
    xslt: "xsl",
};

/** The app ships the TypeScript grammar, and it reads JavaScript well enough to spare a download. */
const SHIPPED_STAND_INS: Readonly<Record<string, string>> = {
    javascript: "typescript",
    jsx: "typescript",
    tsx: "typescript",
};

function known(name: string): string | null {
    if (Object.hasOwn(GRAMMARS, name)) return name;
    return Object.hasOwn(GRAMMAR_ALIASES, name) ? GRAMMAR_ALIASES[name] : null;
}

/** The language a name, file name or path is written in, which may end in `:line` or `:line:column`. */
export function languageOf(nameOrPath: string): string | null {
    const name = (nameOrPath.toLowerCase().split(/[\\/]/).pop() ?? "").replace(/:\d+(?::\d+)?$/, "");
    const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
    return (
        (Object.hasOwn(FILE_NAMES, name) ? FILE_NAMES[name] : null) ??
        (name.startsWith("dockerfile") ? "docker" : null) ??
        (name.startsWith(".env.") ? "dotenv" : null) ??
        known(name) ??
        (Object.hasOwn(EXTENSIONS, extension) ? EXTENSIONS[extension] : null) ??
        (extension ? known(extension) : null)
    );
}

/** The grammar to colour a language with, which is sometimes one the app ships in place of its own. */
export function grammarFor(nameOrPath: string): string | null {
    const id = languageOf(nameOrPath);
    if (!id) return null;
    return Object.hasOwn(SHIPPED_STAND_INS, id) ? SHIPPED_STAND_INS[id] : id;
}
