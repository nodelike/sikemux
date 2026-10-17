import { fsapi } from "../api/fs";
import { joinPath } from "../lib/paths";
import { CARGO_TEST_LOG, GO_TEST_LOG, JEST_REPORT, PYTEST_REPORT, VITEST_REPORT } from "./detectedTasks";

export interface TestFailure {
    readonly className: string;
    readonly name: string;
    readonly message: string;
    /** Absolute path of the test's source file, when it was found. */
    readonly file: string | null;
    /** One-based line the failure was thrown from, when the stack trace names it. */
    readonly line: number | null;
}

export interface TestResults {
    readonly passed: number;
    readonly failed: number;
    readonly skipped: number;
    readonly failures: readonly TestFailure[];
}

/** Where Maven's surefire and Gradle write their JUnit XML reports, relative to the module. */
const REPORT_DIRS = ["target/surefire-reports", "build/test-results/test"];
const TEST_SOURCES = ["src/test/java", "src/test/kotlin"];
const MAX_FAILURES = 50;
/** Jest colours its failure messages for a terminal. */
const TERMINAL_COLOURS = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");

export function isTestTask(taskId: string): boolean {
    return /\.test\b/.test(taskId.split(":")[0]);
}

function escapeRegExp(text: string): string {
    return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function firstLine(text: string): string {
    return (
        text
            .split("\n")
            .map((line) => line.trim())
            .find(Boolean) ?? ""
    );
}

/** The stack frame inside the test class itself, such as `at com.x.FooTest.saves(FooTest.java:57)`. */
function failingFrame(stack: string, className: string): { fileName: string; line: number } | null {
    const frame = new RegExp(`at ${escapeRegExp(className)}\\.[\\w$<>]+\\(([\\w$]+\\.(?:java|kt)):(\\d+)\\)`).exec(stack);
    return frame ? { fileName: frame[1], line: Number(frame[2]) } : null;
}

async function sourceFile(cwd: string, className: string, fileName: string): Promise<string | null> {
    const packagePath = className.split(".").slice(0, -1).join("/");
    const candidates = TEST_SOURCES.map((root) => joinPath(cwd, root, packagePath, fileName));
    const kinds = await fsapi.pathKinds(candidates).catch(() => []);
    return candidates.find((_, index) => kinds[index] === "file") ?? null;
}

/** Vitest names a test case by its file, such as `lib/price.test.ts`, where Java names a class. */
export function isFilePath(className: string): boolean {
    return className.includes("/") || /\.[cm]?[jt]sx?$/.test(className);
}

/** The first `file:line` in a stack trace that points into the test's own file. */
function lineIn(stack: string, file: string): number | null {
    const name = file.split("/").pop() ?? file;
    const match = new RegExp(`${escapeRegExp(name)}:(\\d+)`).exec(stack);
    return match ? Number(match[1]) : null;
}

async function existingFile(path: string): Promise<string | null> {
    const [kind] = await fsapi.pathKinds([path]).catch(() => [null]);
    return kind === "file" ? path : null;
}

async function reportFiles(cwd: string): Promise<string[]> {
    const listings = await Promise.all(REPORT_DIRS.map((dir) => fsapi.readDir(joinPath(cwd, dir)).catch(() => [])));
    const [vitest, pytest] = await Promise.all([existingFile(joinPath(cwd, VITEST_REPORT)), existingFile(joinPath(cwd, PYTEST_REPORT))]);
    return [
        ...listings
            .flat()
            .filter((entry) => !entry.is_dir && /^TEST-.*\.xml$/.test(entry.name))
            .map((entry) => entry.path),
        ...(vitest ? [vitest] : []),
        ...(pytest ? [pytest] : []),
    ];
}

interface JestReport {
    testResults?: {
        name?: string;
        assertionResults?: { status?: string; fullName?: string; title?: string; failureMessages?: string[] }[];
    }[];
}

/** `go test -v` prints a test's log lines between its `=== RUN` and its `--- FAIL`, and the package after its tests. */
async function readGoResults(cwd: string): Promise<TestResults | null> {
    const log = await fsapi.readTextFileLimited(joinPath(cwd, GO_TEST_LOG)).catch(() => null);
    if (!log) return null;
    const module = /^module\s+(\S+)/m.exec(await fsapi.readTextFileLimited(joinPath(cwd, "go.mod")).catch(() => ""))?.[1] ?? "";
    let passed = 0;
    let skipped = 0;
    const failures: TestFailure[] = [];
    let pending: { name: string; message: string; fileName: string | null; line: number | null }[] = [];
    let current: { message: string; fileName: string | null; line: number | null } = { message: "", fileName: null, line: null };
    for (const raw of log.split("\n")) {
        const line = raw.replace(TERMINAL_COLOURS, "");
        if (line.startsWith("=== RUN")) current = { message: "", fileName: null, line: null };
        const logged = /^\s+(\S+\.go):(\d+): (.*)$/.exec(line);
        if (logged && !current.message) current = { fileName: logged[1], line: Number(logged[2]), message: logged[3] };
        const result = /^--- (PASS|FAIL|SKIP): (\S+)/.exec(line);
        if (result?.[1] === "PASS") passed += 1;
        else if (result?.[1] === "SKIP") skipped += 1;
        else if (result?.[1] === "FAIL") pending.push({ name: result[2], ...current });
        const done = /^(?:FAIL|ok)\s+(\S+)/.exec(line);
        if (done && pending.length) {
            const dir = module && done[1].startsWith(module) ? done[1].slice(module.length + 1) : "";
            for (const failure of pending) {
                const file = failure.fileName ? await existingFile(joinPath(cwd, dir, failure.fileName)) : null;
                failures.push({ className: done[1], name: failure.name, message: failure.message, file, line: failure.line });
            }
            pending = [];
        }
    }
    return { passed, failed: failures.length, skipped, failures: failures.slice(0, MAX_FAILURES) };
}

/** `cargo test` names failures in `---- name stdout ----` blocks, and totals each test binary in a `test result:` line. */
async function readCargoResults(cwd: string): Promise<TestResults | null> {
    const log = await fsapi.readTextFileLimited(joinPath(cwd, CARGO_TEST_LOG)).catch(() => null);
    if (!log) return null;
    const text = log.replace(TERMINAL_COLOURS, "");
    let passed = 0;
    let skipped = 0;
    for (const total of text.matchAll(/test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored/g)) {
        passed += Number(total[1]);
        skipped += Number(total[3]);
    }
    const failures: TestFailure[] = [];
    for (const block of text.split(/^---- /m).slice(1)) {
        const name = /^(\S+) stdout ----/.exec(block)?.[1];
        if (!name) continue;
        const panic = /panicked at ([^:\n]+):(\d+):\d+:\n([\s\S]*?)(?:\nnote:|\n\n|$)/.exec(block);
        const message = panic
            ? panic[3]
                  .split("\n")
                  .map((line) => line.trim())
                  .filter(Boolean)
                  .join(" · ")
            : "";
        const file = panic ? await existingFile(joinPath(cwd, panic[1])) : null;
        failures.push({
            className: name.split("::").slice(0, -1).join("::"),
            name: name.split("::").pop() ?? name,
            message,
            file,
            line: panic ? Number(panic[2]) : null,
        });
    }
    return { passed, failed: failures.length, skipped, failures: failures.slice(0, MAX_FAILURES) };
}

/** Jest's first line names the matcher; the values it compared come a few lines later. */
function jestMessage(text: string): string {
    const values = text
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => /^(Expected|Received)\b/.test(line));
    return [firstLine(text), ...values].join(" · ");
}

/** Jest's `--json` report, which the detected test task asks it to write. */
async function readJestResults(cwd: string): Promise<TestResults | null> {
    const text = await fsapi.readTextFileLimited(joinPath(cwd, JEST_REPORT)).catch(() => null);
    if (!text) return null;
    let report: JestReport;
    try {
        report = JSON.parse(text) as JestReport;
    } catch {
        return null;
    }
    let passed = 0;
    let skipped = 0;
    const failures: TestFailure[] = [];
    for (const suite of report.testResults ?? []) {
        const file = suite.name ?? "";
        const className = file.startsWith(`${cwd}/`) ? file.slice(cwd.length + 1) : file;
        for (const test of suite.assertionResults ?? []) {
            if (test.status === "passed") passed += 1;
            else if (test.status === "failed") {
                const stack = (test.failureMessages ?? []).join("\n");
                failures.push({
                    className,
                    name: test.fullName || test.title || "",
                    message: jestMessage(stack.replace(TERMINAL_COLOURS, "")),
                    file: file || null,
                    line: lineIn(stack, file),
                });
            } else skipped += 1;
        }
    }
    return { passed, failed: failures.length, skipped, failures: failures.slice(0, MAX_FAILURES) };
}

/** The latest JUnit XML results in a Maven or Gradle module, or null when it has none. */
export async function readTestResults(cwd: string): Promise<TestResults | null> {
    const [files, jest, go, cargo] = await Promise.all([reportFiles(cwd), readJestResults(cwd), readGoResults(cwd), readCargoResults(cwd)]);
    if (!files.length) return jest ?? go ?? cargo;
    const parser = new DOMParser();
    let passed = 0;
    let skipped = 0;
    const failing: { className: string; name: string; message: string; stack: string }[] = [];
    for (const file of files) {
        const xml = await fsapi.readTextFileLimited(file).catch(() => "");
        const doc = parser.parseFromString(xml, "application/xml");
        for (const testcase of Array.from(doc.getElementsByTagName("testcase"))) {
            const problem = testcase.querySelector("failure, error");
            if (problem) {
                failing.push({
                    className: testcase.getAttribute("classname") ?? "",
                    name: testcase.getAttribute("name") ?? "",
                    message: problem.getAttribute("message") || firstLine(problem.textContent ?? ""),
                    stack: problem.textContent ?? "",
                });
            } else if (testcase.querySelector("skipped")) skipped += 1;
            else passed += 1;
        }
    }
    const failures = await Promise.all(
        failing.slice(0, MAX_FAILURES).map(async ({ className, name, message, stack }): Promise<TestFailure> => {
            if (isFilePath(className)) {
                const file = await existingFile(joinPath(cwd, className));
                return { className, name, message, file, line: lineIn(stack, className) };
            }
            if (/\.py:\d+/.test(stack)) {
                const file = await existingFile(joinPath(cwd, `${className.replace(/\./g, "/")}.py`));
                return { className, name, message, file, line: file ? lineIn(stack, file) : null };
            }
            const frame = failingFrame(stack, className);
            const file = frame ? await sourceFile(cwd, className, frame.fileName) : null;
            return { className, name, message, file, line: frame?.line ?? null };
        }),
    );
    return { passed, failed: failing.length, skipped, failures };
}

/** The same test command narrowed to the tests that failed, or null for a build tool it does not know. */
export function rerunFailedCommand(command: string, failures: readonly TestFailure[]): string | null {
    if (command.includes(PYTEST_REPORT)) return `${command} --lf`;
    if (command.includes(GO_TEST_LOG)) {
        const names = [...new Set(failures.map((failure) => failure.name.split("/")[0]))].join("|");
        return command.replace("go test -v ./...", `go test -v -run "^(${names})\\$" ./...`);
    }
    if (command.includes(CARGO_TEST_LOG)) {
        const names = failures.map((failure) => (failure.className ? `${failure.className}::${failure.name}` : failure.name)).join(" ");
        return command.replace("cargo test", `cargo test -- --exact ${names}`);
    }
    if (command.includes(VITEST_REPORT) || command.includes(JEST_REPORT)) {
        const files = [...new Set(failures.map((failure) => failure.className))].map((file) => `'${file}'`);
        return `${command} ${files.join(" ")}`;
    }
    const [tool] = command.split(" ");
    const methodName = (name: string) => name.replace(/\(.*$/, "");
    if (/mvnw?$/.test(tool)) {
        const byClass = new Map<string, string[]>();
        for (const failure of failures) {
            const simple = failure.className.split(".").pop()!;
            byClass.set(simple, [...(byClass.get(simple) ?? []), methodName(failure.name)]);
        }
        const selector = [...byClass].map(([cls, methods]) => `${cls}#${methods.join("+")}`).join(",");
        return `${tool} test -Dtest='${selector}' -Dsurefire.failIfNoSpecifiedTests=false`;
    }
    if (/gradlew?$/.test(tool)) {
        const filters = failures.map((failure) => `--tests '${failure.className}.${methodName(failure.name)}'`).join(" ");
        return `${tool} test ${filters}`;
    }
    return null;
}
