export function FileStatus({ code }: { code: string }) {
    const letter = code === "?" ? "U" : code.trim();
    const cls = letter === "A" || letter === "U" ? "added" : letter === "D" ? "deleted" : letter === "R" || letter === "C" ? "renamed" : "modified";
    return <span className={`git-status ${cls}`}>{letter}</span>;
}
