import { fsapi, type FileSnapshot } from "../api/fs";

/**
 * Owns editor file versions and serializes writes for each canonical path.
 * Native compare-and-swap remains authoritative; this queue prevents two
 * saves from this renderer from racing each other before they cross IPC.
 */
export class DocumentIO {
    private readonly documents = new Map<string, { version?: string }>();
    private readonly queues = new Map<string, Promise<unknown>>();

    async read(path: string): Promise<FileSnapshot> {
        const document = this.document(path);
        return this.enqueue(path, async () => {
            const snapshot = await this.peek(path);
            if (this.documents.get(path) === document) document.version = snapshot.version;
            return snapshot;
        });
    }

    peek(path: string): Promise<FileSnapshot> {
        return fsapi.readFileVersioned(path);
    }

    observe(path: string, snapshot: FileSnapshot): void {
        this.document(path).version = snapshot.version;
    }

    version(path: string): string | undefined {
        return this.documents.get(path)?.version;
    }

    changedSinceObserved(path: string, snapshot: FileSnapshot): boolean {
        const observed = this.documents.get(path)?.version;
        return observed !== undefined && observed !== snapshot.version;
    }

    save(path: string, content: string): Promise<FileSnapshot> {
        const document = this.document(path);
        return this.enqueue(path, async () => {
            if (this.documents.get(path) !== document) throw new Error(`${path} was closed before its queued save started`);
            const expectedVersion = document.version;
            if (!expectedVersion) throw new Error(`No file version is available for ${path}; reload it before saving`);
            const result = await fsapi.writeFileVersioned(path, content, expectedVersion);
            const snapshot = { content, version: result.version };
            if (this.documents.get(path) === document) document.version = snapshot.version;
            return snapshot;
        });
    }

    relocate(src: string, dest: string): void {
        const document = this.documents.get(src);
        this.documents.delete(src);
        if (document) this.documents.set(dest, document);
    }

    forget(path: string): void {
        this.documents.delete(path);
    }

    private document(path: string): { version?: string } {
        let document = this.documents.get(path);
        if (!document) {
            document = {};
            this.documents.set(path, document);
        }
        return document;
    }

    private enqueue<T>(path: string, task: () => Promise<T>): Promise<T> {
        const previous = this.queues.get(path) ?? Promise.resolve();
        const operation = previous.catch(() => undefined).then(task);
        this.queues.set(path, operation);
        const cleanup = () => {
            if (this.queues.get(path) === operation) this.queues.delete(path);
        };
        void operation.then(cleanup, cleanup);
        return operation;
    }
}
