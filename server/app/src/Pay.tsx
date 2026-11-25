import { connectApi } from "@shared/account/api";
import { useEffect, useState } from "react";

import { problemText } from "./api.ts";
import { config } from "./config.ts";
import { Foot } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { BILLING } from "./navigation.ts";
import { Btn, btnClass, Hero, Problem } from "./ui.tsx";
import "./billing.css";

const PADDLE_JS = "https://cdn.paddle.com/paddle/v2/paddle.js";

interface PaddleEvent {
  name?: string;
}

interface Paddle {
  Environment: { set(environment: "sandbox"): void };
  Initialize(options: {
    token: string;
    checkout: { settings: { successUrl: string; allowLogout: boolean } };
    eventCallback: (event: PaddleEvent) => void;
  }): void;
}

declare global {
  interface Window {
    Paddle?: Paddle;
  }
}

function loadPaddle(): Promise<Paddle> {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = PADDLE_JS;
    script.onload = () =>
      window.Paddle ? resolve(window.Paddle) : reject(new Error());
    script.onerror = () =>
      reject(new Error("Paddle's checkout didn't load. Try again."));
    document.head.append(script);
  });
}

/**
 * Paddle sends every checkout here with `?_ptxn=` and the transaction the API made. Paddle.js
 * reads it and opens the checkout over this page; finishing or closing it goes back to billing.
 */
export function Pay() {
  const [problem, setProblem] = useState<string | null>(null);
  const back = `${location.origin}${BILLING}`;

  useEffect(() => {
    if (!new URLSearchParams(location.search).has("_ptxn")) {
      location.replace(back);
      return;
    }
    const routes = connectApi({
      url: config.apiUrl,
      offline: "Can't reach Sikemux. Check your connection and try again.",
    });
    Promise.all([routes.getPaddleCheckout(), loadPaddle()])
      .then(([checkout, paddle]) => {
        if (checkout.environment === "sandbox")
          paddle.Environment.set("sandbox");
        paddle.Initialize({
          token: checkout.clientToken,
          checkout: { settings: { successUrl: back, allowLogout: false } },
          eventCallback: (event) => {
            if (event.name === "checkout.closed") location.replace(back);
          },
        });
      })
      .catch((error: unknown) =>
        setProblem(`${problemText(error)} Nothing was charged.`),
      );
  }, [back]);

  return (
    <Hero
      title="Checkout"
      lede={
        problem
          ? "Paddle's checkout didn't load, so nothing was charged."
          : "Opening Paddle's checkout…"
      }
      mark={iconMark("card")}
      scene="plan"
      tone={problem ? "failed" : "waiting"}
      foot={<Foot />}
    >
      {problem ? (
        <>
          <Problem>{problem}</Problem>
          <div className="pay-do">
            <Btn onClick={() => location.reload()}>Try again</Btn>
            <a className={btnClass({ quiet: true })} href={back}>
              Back to billing
            </a>
          </div>
        </>
      ) : null}
    </Hero>
  );
}
