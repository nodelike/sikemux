import { fsapi, type DirEntry } from "../api/fs";
import { joinPath } from "../lib/paths";
import type { TaskDefinitionInput } from "./taskRegistry";

interface DetectedTask {
    readonly id: string;
    readonly label: string;
    readonly command: string;
    /** The Java major version the project's build files ask for. */
    readonly java?: number;
}

export const DETECTED_TASK_PREFIX = "detected.";

export interface DetectedTaskDefinition extends TaskDefinitionInput {
    readonly java?: number;
}

type ReadText = (path: string) => Promise<string>;
type ReadDir = (path: string) => Promise<DirEntry[]>;

async function readOrEmpty(read: ReadText, path: string): Promise<string> {
    try {
        return await read(path);
    } catch {
        return "";
    }
}

function firstNumber(text: string, patterns: readonly RegExp[]): number | undefined {
    for (const pattern of patterns) {
        const match = pattern.exec(text);
        if (match) return Number(match[1]);
    }
    return undefined;
}

/** Spring Boot 3 and later need Java 17 even when the build names no version. */
function springBootMinimum(build: string): number | undefined {
    const boot = firstNumber(build, [
        /spring-boot-starter-parent<\/artifactId>\s*<version>(\d+)/,
        /org\.springframework\.boot["']?\)?\s*version\s*["'(]*(\d+)/,
    ]);
    return boot !== undefined && boot >= 3 ? 17 : undefined;
}

function javaFromPom(pom: string): number | undefined {
    return (
        firstNumber(pom, [
            /<maven\.compiler\.release>(\d+)</,
            /<release>(\d+)<\/release>/,
            /<java\.version>(?:1\.)?(\d+)</,
            /<maven\.compiler\.source>(?:1\.)?(\d+)</,
        ]) ?? springBootMinimum(pom)
    );
}

function javaFromGradle(build: string): number | undefined {
    return (
        firstNumber(build, [
            /JavaLanguageVersion\.of\(\s*(\d+)/,
            /sourceCompatibility\s*=?\s*JavaVersion\.VERSION_(?:1_)?(\d+)/,
            /sourceCompatibility\s*=?\s*['"](?:1\.)?(\d+)/,
            /jvmTarget\s*=?\s*(?:JvmTarget\.JVM_|['"])(?:1_|1\.)?(\d+)/,
        ]) ?? springBootMinimum(build)
    );
}

/** What a developer pinned with jenv, SDKMAN or asdf/mise wins over the build file's minimum. */
async function javaFromVersionFiles(dir: string, files: ReadonlySet<string>, read: ReadText): Promise<number | undefined> {
    if (files.has(".java-version")) {
        const pinned = firstNumber(await readOrEmpty(read, joinPath(dir, ".java-version")), [/(?:^|-)(?:1\.)?(\d+)/]);
        if (pinned) return pinned;
    }
    if (files.has(".sdkmanrc")) {
        const pinned = firstNumber(await readOrEmpty(read, joinPath(dir, ".sdkmanrc")), [/^\s*java\s*=\s*(\d+)/m]);
        if (pinned) return pinned;
    }
    if (files.has(".tool-versions")) {
        const pinned = firstNumber(await readOrEmpty(read, joinPath(dir, ".tool-versions")), [/^\s*java\s+(?:[a-z]+-)*(\d+)/m]);
        if (pinned) return pinned;
    }
    return undefined;
}

async function mavenTasks(root: string, files: ReadonlySet<string>, read: ReadText): Promise<DetectedTask[]> {
    if (!files.has("pom.xml")) return [];
    const mvn = files.has("mvnw") ? "./mvnw" : "mvn";
    const pom = await readOrEmpty(read, joinPath(root, "pom.xml"));
    const java = (await javaFromVersionFiles(root, files, read)) ?? javaFromPom(pom);
    const tasks: DetectedTask[] = [
        ...(pom.includes("spring-boot")
            ? [{ id: "detected.maven.spring-boot-run", label: "Spring Boot: run", command: `${mvn} spring-boot:run` }]
            : []),
        { id: "detected.maven.test", label: "Maven: test", command: `${mvn} test` },
        { id: "detected.maven.package", label: "Maven: package", command: `${mvn} package` },
    ];
    return tasks.map((task) => ({ ...task, java }));
}

async function gradleTasks(root: string, files: ReadonlySet<string>, read: ReadText): Promise<DetectedTask[]> {
    const buildFile = ["build.gradle.kts", "build.gradle"].find((name) => files.has(name));
    if (!buildFile) return [];
    const gradle = files.has("gradlew") ? "./gradlew" : "gradle";
    const build = await readOrEmpty(read, joinPath(root, buildFile));
    const java = (await javaFromVersionFiles(root, files, read)) ?? javaFromGradle(build);
    const tasks: DetectedTask[] = [
        ...(build.includes("org.springframework.boot")
            ? [{ id: "detected.gradle.boot-run", label: "Spring Boot: bootRun", command: `${gradle} bootRun` }]
            : []),
        ...(/\bapplication\b/.test(build) ? [{ id: "detected.gradle.run", label: "Gradle: run", command: `${gradle} run` }] : []),
        { id: "detected.gradle.test", label: "Gradle: test", command: `${gradle} test` },
        { id: "detected.gradle.build", label: "Gradle: build", command: `${gradle} build` },
    ];
    return tasks.map((task) => ({ ...task, java }));
}

async function goTasks(root: string, files: ReadonlySet<string>, readDir: ReadDir): Promise<DetectedTask[]> {
    if (!files.has("go.mod")) return [];
    const commands = files.has("cmd/") ? await readDir(joinPath(root, "cmd")).catch(() => []) : [];
    return [
        ...(files.has("main.go") ? [{ id: "detected.go.run", label: "Go: run", command: "go run ." }] : []),
        ...commands
            .filter((entry) => entry.is_dir && /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(entry.name))
            .map((entry) => ({ id: `detected.go.run.${entry.name}`, label: `Go: run ${entry.name}`, command: `go run ./cmd/${entry.name}` })),
        { id: "detected.go.test", label: "Go: test", command: "go test ./..." },
        { id: "detected.go.build", label: "Go: build", command: "go build ./..." },
    ];
}

const SKIPPED_FOLDERS = new Set(["node_modules", "target", "build", "dist", "out", "vendor"]);
const FOLDER_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

async function tasksIn(dir: string, readDir: ReadDir, read: ReadText): Promise<DetectedTask[]> {
    const entries = await readDir(dir).catch(() => []);
    const files = new Set(entries.map((entry) => (entry.is_dir ? `${entry.name}/` : entry.name)));
    return (await Promise.all([mavenTasks(dir, files, read), gradleTasks(dir, files, read), goTasks(dir, files, readDir)])).flat();
}

/** Run, test and build tasks suggested by the build files at the project root and in the folders just inside it. */
export async function detectProjectTasks(
    root: string,
    readDir: ReadDir = fsapi.readDir,
    read: ReadText = fsapi.readTextFileLimited,
): Promise<readonly DetectedTaskDefinition[]> {
    const folders = (await readDir(root).catch(() => [])).filter(
        (entry) => entry.is_dir && FOLDER_NAME.test(entry.name) && !SKIPPED_FOLDERS.has(entry.name),
    );
    const [atRoot, ...inFolders] = await Promise.all([
        tasksIn(root, readDir, read),
        ...folders.map((folder) => tasksIn(joinPath(root, folder.name), readDir, read)),
    ]);
    return [
        ...atRoot.map((task) => ({ ...task, project: root, cwd: root })),
        ...inFolders.flatMap((tasks, index) => {
            const folder = folders[index].name;
            return tasks.map((task) => ({
                ...task,
                id: `${task.id}:${folder}`,
                label: `${task.label} (${folder})`,
                project: root,
                cwd: joinPath(root, folder),
            }));
        }),
    ];
}

export function isDetectedRunTask(taskId: string): boolean {
    return /\brun\b/.test(taskId.split(":")[0]);
}

const BUILD_TOOL_NAMES: Readonly<Record<string, string>> = { mvn: "Maven", gradle: "Gradle", go: "Go" };

/** The installed program a detected task's command starts with; a project's own `./mvnw` or `./gradlew` needs none. */
export function buildToolOf(command: string): { program: string; name: string } | null {
    const program = command.split(" ")[0];
    return Object.hasOwn(BUILD_TOOL_NAMES, program) ? { program, name: BUILD_TOOL_NAMES[program] } : null;
}
