import { fsapi } from "../api/fs";
import { joinPath } from "../lib/paths";

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

async function reportFiles(cwd: string): Promise<string[]> {
    const listings = await Promise.all(REPORT_DIRS.map((dir) => fsapi.readDir(joinPath(cwd, dir)).catch(() => [])));
    return listings
        .flat()
        .filter((entry) => !entry.is_dir && /^TEST-.*\.xml$/.test(entry.name))
        .map((entry) => entry.path);
}

/** The latest JUnit XML results in a Maven or Gradle module, or null when it has none. */
export async function readTestResults(cwd: string): Promise<TestResults | null> {
    const files = await reportFiles(cwd);
    if (!files.length) return null;
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
            const frame = failingFrame(stack, className);
            const file = frame ? await sourceFile(cwd, className, frame.fileName) : null;
            return { className, name, message, file, line: frame?.line ?? null };
        }),
    );
    return { passed, failed: failing.length, skipped, failures };
}

/** The same test command narrowed to the tests that failed, or null for a build tool it does not know. */
export function rerunFailedCommand(command: string, failures: readonly TestFailure[]): string | null {
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
