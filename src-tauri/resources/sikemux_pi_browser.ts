import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";

type JsonObject = Record<string, unknown>;
type PendingRequest = {
  resolve: (value: JsonObject) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

class McpClient {
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<number, PendingRequest>();
  private nextId = 1;

  constructor(command: string, args: string[]) {
    this.child = spawn(command, args, {
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.stderr.pipe(process.stderr);
    createInterface({ input: this.child.stdout }).on("line", (line) =>
      this.receive(line),
    );
    this.child.on("error", (error) => this.fail(error));
    this.child.on("exit", (code, signal) =>
      this.fail(
        new Error(
          `sikemux-browser exited with ${signal ?? code ?? "unknown status"}`,
        ),
      ),
    );
  }

  private receive(line: string): void {
    let message: JsonObject;
    try {
      message = JSON.parse(line) as JsonObject;
    } catch {
      return;
    }
    if (typeof message.id !== "number") return;
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    clearTimeout(pending.timer);
    if (message.error && typeof message.error === "object") {
      const error = message.error as JsonObject;
      pending.reject(
        new Error(
          typeof error.message === "string"
            ? error.message
            : "MCP request failed",
        ),
      );
      return;
    }
    pending.resolve((message.result as JsonObject | undefined) ?? {});
  }

  private fail(error: Error): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
  }

  notify(method: string, params: JsonObject = {}): void {
    this.child.stdin.write(
      `${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`,
    );
  }

  request(
    method: string,
    params: JsonObject = {},
    signal?: AbortSignal,
  ): Promise<JsonObject> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const abort = () => {
        this.pending.delete(id);
        clearTimeout(timer);
        this.notify("notifications/cancelled", {
          requestId: id,
          reason: "cancelled by Pi",
        });
        reject(
          signal?.reason instanceof Error
            ? signal.reason
            : new Error("MCP request cancelled"),
        );
      };
      const timer = setTimeout(() => {
        this.pending.delete(id);
        signal?.removeEventListener("abort", abort);
        reject(new Error(`MCP ${method} timed out`));
      }, 75_000);
      this.pending.set(id, {
        resolve: (value) => {
          signal?.removeEventListener("abort", abort);
          resolve(value);
        },
        reject: (error) => {
          signal?.removeEventListener("abort", abort);
          reject(error);
        },
        timer,
      });
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) {
        clearTimeout(timer);
        abort();
        return;
      }
      this.child.stdin.write(
        `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
      );
    });
  }

  stop(): void {
    this.child.kill();
  }
}

function contentFrom(
  result: JsonObject,
): Array<
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string }
> {
  if (!Array.isArray(result.content))
    return [{ type: "text", text: JSON.stringify(result) }];
  return result.content.flatMap((block) => {
    if (!block || typeof block !== "object") return [];
    const value = block as JsonObject;
    if (value.type === "text" && typeof value.text === "string")
      return [{ type: "text" as const, text: value.text }];
    if (
      value.type === "image" &&
      typeof value.data === "string" &&
      typeof value.mimeType === "string"
    ) {
      return [
        { type: "image" as const, data: value.data, mimeType: value.mimeType },
      ];
    }
    return [];
  });
}

export default async function sikemuxBrowser(pi: ExtensionAPI): Promise<void> {
  const command = process.env.SIKEMUX_BROWSER_MCP_COMMAND;
  if (!command) return;
  let args: string[];
  try {
    const parsed = JSON.parse(
      process.env.SIKEMUX_BROWSER_MCP_ARGS ?? "[]",
    ) as unknown;
    if (
      !Array.isArray(parsed) ||
      parsed.some((value) => typeof value !== "string")
    )
      throw new Error("invalid MCP arguments");
    args = parsed;
  } catch (error) {
    console.error(
      `[sikemux-browser] ${error instanceof Error ? error.message : String(error)}`,
    );
    return;
  }

  const client = new McpClient(command, args);
  try {
    await client.request("initialize", {
      protocolVersion: "2025-03-26",
      capabilities: {},
      clientInfo: { name: "sikemux-pi", version: "1" },
    });
    client.notify("notifications/initialized");
    const listed = await client.request("tools/list");
    const tools = Array.isArray(listed.tools) ? listed.tools : [];
    for (const rawTool of tools) {
      if (!rawTool || typeof rawTool !== "object") continue;
      const tool = rawTool as JsonObject;
      if (typeof tool.name !== "string") continue;
      const name = tool.name;
      const description =
        typeof tool.description === "string"
          ? tool.description
          : `Sikemux browser tool ${name}`;
      const inputSchema =
        tool.inputSchema && typeof tool.inputSchema === "object"
          ? (tool.inputSchema as JsonObject)
          : { type: "object" };
      pi.registerTool({
        name,
        label: name.replaceAll("_", " "),
        description,
        promptSnippet: description,
        parameters: Type.Unsafe(inputSchema),
        async execute(_toolCallId, params, signal) {
          const result = await client.request(
            "tools/call",
            { name, arguments: params as JsonObject },
            signal,
          );
          const content = contentFrom(result);
          if (result.isError === true) {
            throw new Error(
              content
                .map((block) =>
                  block.type === "text" ? block.text : "browser image error",
                )
                .join("\n"),
            );
          }
          return {
            content,
            details: { server: "sikemux-browser", tool: name },
          };
        },
      });
    }
  } catch (error) {
    client.stop();
    console.error(
      `[sikemux-browser] ${error instanceof Error ? error.message : String(error)}`,
    );
    return;
  }
  pi.on("session_shutdown", async () => client.stop());
}
