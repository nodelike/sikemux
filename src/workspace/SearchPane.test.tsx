import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SearchFile, SearchResults } from "../api/search";

const project = vi.fn();
vi.mock("../api/search", () => ({
    searchApi: {
        project: (...args: unknown[]) => project(...args),
        cancel: vi.fn(async () => {}),
        replace: vi.fn(),
        readFileWindow: vi.fn(async () => ({ doc: "", start_line: 1, total_lines: 0, clipped_head: false, clipped_tail: false })),
    },
}));

import { SearchPane } from "./SearchPane";
import * as cmd from "../state/commands";

afterEach(() => {
    cleanup();
    project.mockReset();
    vi.useRealTimers();
});

function resultFile(index: number): SearchFile {
    return { path: `src/file-${index}.ts`, matches: [{ line: 1, text: "needle", ranges: [{ start: 0, end: 6 }] }] };
}

describe("SearchPane", () => {
    it("flushes streamed result files in batches instead of one render per file", async () => {
        vi.useFakeTimers();
        let stream: ((file: SearchFile) => void) | null = null;
        let finish: ((results: SearchResults) => void) | null = null;
        project.mockImplementation((_repo: string, _query: string, _options: unknown, onFile: (file: SearchFile) => void) => {
            stream = onFile;
            return new Promise<SearchResults>((resolve) => {
                finish = resolve;
            });
        });

        cmd.setGlobalSearchQuery("search-batching", "needle");
        const { container } = render(<SearchPane sessionId="search-batching" cwd="/repo" active visible compact />);
        const listedRows = () => {
            const spacer = container.querySelector<HTMLElement>(".sp-threads-spacer");
            return spacer ? Number.parseInt(spacer.style.height, 10) || 0 : 0;
        };
        await act(async () => {
            vi.advanceTimersByTime(300);
        });
        expect(project).toHaveBeenCalledTimes(1);

        act(() => {
            for (let index = 0; index < 200; index++) stream!(resultFile(index));
        });
        expect(listedRows()).toBe(0);

        act(() => {
            vi.advanceTimersByTime(60);
        });
        expect(listedRows()).toBeGreaterThan(0);

        await act(async () => {
            finish!({ files: [], file_count: 200, match_count: 200, truncated: false, elapsed_ms: 3 });
        });
        expect(listedRows()).toBeGreaterThan(0);
        expect(container.querySelector(".sp-stats")?.textContent).toContain("200");
    });
});
