import {
  validator,
  type ApiError,
  type Definitions,
  type ErrorCode,
  type ErrorDetail,
} from "@sikemux/protocol";
import type { Context, MiddlewareHandler } from "hono";
import type { ContentfulStatusCode } from "hono/utils/http-status";

import type { Logger } from "./log.ts";

export interface Env {
  Variables: {
    requestId: string;
    log: Logger;
  };
}

/** What an error says beyond its code and message, such as how recently to verify again. */
export type ErrorExtras = Omit<ErrorDetail, "code" | "message" | "requestId">;

/** Thrown by a handler to answer with an error the caller can act on. */
export class ApiFailure extends Error {
  readonly status: ContentfulStatusCode;
  readonly code: ErrorCode;
  readonly extras: ErrorExtras;

  constructor(
    status: ContentfulStatusCode,
    code: ErrorCode,
    message: string,
    extras: ErrorExtras = {},
  ) {
    super(message);
    this.status = status;
    this.code = code;
    this.extras = extras;
  }
}

/** The body of every error the API answers, HTTP or a refused WebSocket upgrade. */
export function errorBody(
  requestId: string,
  code: ErrorCode,
  message: string,
  extras: ErrorExtras = {},
): ApiError {
  return { error: { code, message, requestId, ...extras } };
}

export function errorResponse(
  c: Context<Env>,
  status: ContentfulStatusCode,
  code: ErrorCode,
  message: string,
  extras: ErrorExtras = {},
) {
  return c.json(errorBody(c.get("requestId"), code, message, extras), status);
}

/** Gives each request an id, a logger that carries it, and one log line when it finishes. */
export function requestContext(log: Logger): MiddlewareHandler<Env> {
  return async (c, next) => {
    const requestId = crypto.randomUUID();
    const started = performance.now();
    c.set("requestId", requestId);
    c.set("log", log.child({ requestId }));
    c.header("x-request-id", requestId);
    await next();
    c.get("log").info(
      {
        method: c.req.method,
        path: c.req.path,
        status: c.res.status,
        ms: Math.round(performance.now() - started),
      },
      "request",
    );
  };
}

const protocol = validator();

function checked<Name extends keyof Definitions & string>(
  name: Name,
  text: string,
): Definitions[Name] {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new ApiFailure(400, "bad_request", "The body is not JSON.");
  }
  const result = protocol.validate(name, value);
  if (!result.ok)
    throw new ApiFailure(
      400,
      "bad_request",
      `The body is not a valid ${name}: ${result.problems.join("; ")}.`,
    );
  return result.value;
}

/** Reads a JSON body and checks it against the named protocol definition before any handler sees it. */
export async function readBody<Name extends keyof Definitions & string>(
  c: Context,
  name: Name,
): Promise<Definitions[Name]> {
  return checked(name, await c.req.text());
}

/** Like readBody, for routes whose body may be left out; undefined when it was. */
export async function readOptionalBody<Name extends keyof Definitions & string>(
  c: Context,
  name: Name,
): Promise<Definitions[Name] | undefined> {
  const text = await c.req.text();
  return text.trim() ? checked(name, text) : undefined;
}

export function badRequest(message: string): ApiFailure {
  return new ApiFailure(400, "bad_request", message);
}

export function unauthorized(message: string): ApiFailure {
  return new ApiFailure(401, "unauthorized", message);
}

export function forbidden(message: string): ApiFailure {
  return new ApiFailure(403, "forbidden", message);
}

export function notFound(message: string): ApiFailure {
  return new ApiFailure(404, "not_found", message);
}
