import type { Logger } from "../log.ts";
import { PRODUCTS } from "./catalog.ts";
import {
  CLIENT_TOKEN_NAME,
  PADDLE_EVENTS,
  PAY_PATH,
  priceTag,
  WEBHOOK_PATH,
  type PaddleApi,
  type PaddlePrice,
} from "./paddle-api.ts";

interface PaddleProduct {
  id: string;
  name: string;
  custom_data: Record<string, unknown> | null;
}

interface NotificationSetting {
  id: string;
  destination: string;
  active: boolean;
  traffic_source: string;
  subscribed_events: ({ name: string } | string)[];
}

const INTERVAL_NAMES = { month: "Monthly", year: "Yearly" } as const;

/**
 * Makes Paddle hold what the catalog sells, and safe to run again: a product per catalog product,
 * a price per interval at the catalog's amount, the webhook destination, the checkout's client
 * token and the default payment link, the app's /pay page. A live account's link must be on a
 * domain Paddle has approved, which is asked for in its dashboard. A price whose amount or limits changed is archived for a new one, so subscriptions
 * already on it keep it.
 */
export async function setUpPaddle(
  api: PaddleApi,
  { apiOrigin, appOrigin }: { apiOrigin: string; appOrigin: string },
  log: Logger,
) {
  const account = await api.request<{ id: string; legal_name: string }>(
    "GET",
    "/account",
  );
  log.info(
    {
      account: account.id,
      name: account.legal_name,
      environment: api.sandbox ? "sandbox" : "live",
    },
    "setting up this Paddle account",
  );
  const trafficSource = api.sandbox ? "all" : "platform";
  const products = await api.list<PaddleProduct>(
    "/products?status=active&per_page=200",
  );
  const prices = await api.list<PaddlePrice>(
    "/prices?status=active&per_page=200",
  );

  for (const item of PRODUCTS) {
    const wanted = {
      name: `Sikemux ${item.name}`,
      tax_category: "saas",
      custom_data: { product: item.id },
    };
    let product = products.find(
      (each) => each.custom_data?.product === item.id,
    );
    if (!product) {
      product = await api.request<PaddleProduct>("POST", "/products", wanted);
      log.info({ product: item.id, id: product.id }, "made a Paddle product");
    } else if (product.name !== wanted.name) {
      await api.request("PATCH", `/products/${product.id}`, {
        name: wanted.name,
      });
      log.info({ product: item.id }, "renamed a Paddle product");
    }

    for (const { interval, unit } of item.prices) {
      const tagged = prices.filter((price) => {
        const tag = priceTag(price);
        return tag?.product === item.id && tag.interval === interval;
      });
      const current = tagged.find(
        (price) =>
          price.product_id === product.id &&
          Number(price.unit_price.amount) === unit.amount &&
          price.unit_price.currency_code.toLowerCase() === unit.currency &&
          price.billing_cycle?.interval === interval &&
          price.billing_cycle.frequency === 1 &&
          price.quantity.minimum === 1 &&
          price.quantity.maximum === item.maxQuantity,
      );
      if (!current) {
        const made = await api.request<PaddlePrice>("POST", "/prices", {
          product_id: product.id,
          name: INTERVAL_NAMES[interval],
          description: `${item.name}, ${INTERVAL_NAMES[interval].toLowerCase()}`,
          billing_cycle: { interval, frequency: 1 },
          unit_price: {
            amount: String(unit.amount),
            currency_code: unit.currency.toUpperCase(),
          },
          quantity: { minimum: 1, maximum: item.maxQuantity },
          custom_data: { product: item.id, interval },
        });
        log.info(
          { product: item.id, interval, amount: unit.amount, id: made.id },
          "made a Paddle price",
        );
      }
      for (const old of tagged.filter((price) => price !== current)) {
        await api.request("PATCH", `/prices/${old.id}`, { status: "archived" });
        log.info(
          { product: item.id, interval, id: old.id },
          "archived a Paddle price the catalog no longer sets",
        );
      }
    }
  }

  const destination = apiOrigin + WEBHOOK_PATH;
  const settings = await api.list<NotificationSetting>(
    "/notification-settings",
  );
  const setting = settings.find((each) => each.destination === destination);
  const events = [...PADDLE_EVENTS].sort();
  if (!setting) {
    await api.request("POST", "/notification-settings", {
      description: "Sikemux accounts API",
      type: "url",
      destination,
      api_version: 1,
      traffic_source: trafficSource,
      subscribed_events: events,
    });
    log.info({ destination }, "made Paddle's webhook destination");
  } else {
    const subscribed = setting.subscribed_events
      .map((event) => (typeof event === "string" ? event : event.name))
      .sort();
    if (
      !setting.active ||
      setting.traffic_source !== trafficSource ||
      subscribed.join() !== events.join()
    ) {
      await api.request("PATCH", `/notification-settings/${setting.id}`, {
        active: true,
        traffic_source: trafficSource,
        subscribed_events: events,
      });
      log.info({ destination }, "updated Paddle's webhook destination");
    }
  }

  const tokens = await api.list<{ name: string }>(
    "/client-tokens?status=active",
  );
  if (!tokens.some((token) => token.name === CLIENT_TOKEN_NAME)) {
    await api.request("POST", "/client-tokens", {
      name: CLIENT_TOKEN_NAME,
      description: "Opens Paddle's checkout on the web app's /pay page.",
    });
    log.info("made Paddle's client token for the checkout page");
  }

  const payPage = appOrigin + PAY_PATH;
  const checkoutUrl = await api.request<{ default_url: string | null }>(
    "GET",
    "/settings/checkout-url",
  );
  if (checkoutUrl.default_url !== payPage) {
    await api.request("PATCH", "/settings/checkout-url", {
      default_url: payPage,
    });
    log.info({ payPage }, "set Paddle's default payment link");
  }

  log.info(
    { destination, payPage },
    "Paddle holds the catalog, the webhook destination, the client token and the payment link",
  );
}
