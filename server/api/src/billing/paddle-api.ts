import type { BillingInterval, BillingProductId } from "@sikemux/protocol";

import type { Logger } from "../log.ts";
import { PRODUCTS } from "./catalog.ts";

/** Where Paddle sends its events: the API's webhook route. */
export const WEBHOOK_PATH = "/v1/billing/provider";

/** The client token the checkout page opens Paddle's checkout with, found by this name. */
export const CLIENT_TOKEN_NAME = "app.sikemux.com checkout";

/** The events the webhook destination subscribes to: the ones the adapter reads. */
export const PADDLE_EVENTS = [
  "subscription.updated",
  "subscription.resumed",
  "subscription.past_due",
  "subscription.canceled",
  "transaction.completed",
  "transaction.payment_failed",
  "adjustment.created",
  "adjustment.updated",
] as const;

export const isSandboxKey = (key: string) => key.startsWith("pdl_sdbx_");

/** A Paddle API key, in the shape Paddle issues them. */
export const PADDLE_KEY = /^pdl_(sdbx|live)_apikey_[A-Za-z0-9_]+$/;

export class PaddleError extends Error {
  readonly status: number;
  readonly code: string | null;

  constructor(status: number, code: string | null, detail: string) {
    super(`Paddle answered ${status}${code ? ` ${code}` : ""}: ${detail}`);
    this.status = status;
    this.code = code;
  }
}

export interface PaddlePrice {
  id: string;
  product_id: string;
  status: "active" | "archived";
  billing_cycle: { interval: string; frequency: number } | null;
  unit_price: { amount: string; currency_code: string };
  quantity: { minimum: number; maximum: number };
  custom_data: Record<string, unknown> | null;
}

/** What the setup command writes on each price, which is how a price is found again. */
export interface PriceTag {
  product: BillingProductId;
  interval: BillingInterval;
}

export function priceTag(
  price: Pick<PaddlePrice, "custom_data">,
): PriceTag | null {
  const product = PRODUCTS.find(
    (each) => each.id === price.custom_data?.product,
  );
  const interval = price.custom_data?.interval;
  if (!product || (interval !== "month" && interval !== "year")) return null;
  return { product: product.id, interval };
}

const TIMEOUT_MS = 15_000;

interface Answer {
  data?: unknown;
  meta?: { pagination?: { next?: string; has_more?: boolean } };
  error?: { code?: string; detail?: string };
}

function answerOf(text: string): Answer {
  try {
    return text ? (JSON.parse(text) as Answer) : {};
  } catch {
    return {};
  }
}

/** Paddle's API, reached with one key. The key decides sandbox or live, and is never logged. */
export class PaddleApi {
  readonly sandbox: boolean;
  private readonly base: string;
  private readonly key: string;
  private readonly log: Logger;

  constructor(key: string, log: Logger) {
    this.key = key;
    this.log = log;
    this.sandbox = isSandboxKey(key);
    this.base = this.sandbox
      ? "https://sandbox-api.paddle.com"
      : "https://api.paddle.com";
  }

  private async send(method: string, path: string, body?: unknown) {
    const response = await fetch(this.base + path, {
      method,
      headers: {
        authorization: `Bearer ${this.key}`,
        "content-type": "application/json",
        "paddle-version": "1",
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    const parsed = answerOf(await response.text());
    if (response.ok) return parsed;
    const code = parsed.error?.code ?? null;
    if (response.status === 401 || response.status === 403)
      this.log.error(
        { status: response.status, code, method, path },
        "PADDLE REFUSED THE API KEY: it has expired, been revoked or lacks a permission. Every billing call fails until PADDLE_API_KEY is replaced.",
      );
    throw new PaddleError(
      response.status,
      code,
      parsed.error?.detail ?? response.statusText,
    );
  }

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    return (await this.send(method, path, body)).data as T;
  }

  /** Every page of a list. */
  async list<T>(path: string): Promise<T[]> {
    const all: T[] = [];
    let next: string | null = path;
    while (next) {
      const page = await this.send("GET", next);
      all.push(...((page.data as T[]) ?? []));
      const more = page.meta?.pagination;
      next =
        more?.has_more && more.next
          ? more.next.slice(new URL(more.next).origin.length)
          : null;
    }
    return all;
  }
}
