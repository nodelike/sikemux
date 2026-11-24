import lockup from "../../../brand/wordmark/lockup-purple.svg";

import { brandMark } from "./marks.ts";
import { Hero } from "./ui.tsx";

export function Foot() {
  return (
    <>
      <img src={lockup} alt="Sikemux" />
      <a href="https://sikemux.com/privacy">Privacy</a>
      <a href="https://sikemux.com/terms">Terms</a>
    </>
  );
}

export function Loading({ title = "Loading…" }: { title?: string }) {
  return <Hero title={title} mark={brandMark} scene="sign" tone="waiting" />;
}
