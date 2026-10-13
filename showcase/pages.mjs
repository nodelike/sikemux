// Real public pages an agent's browser tab shows, snapshotted once and committed so captures stay the same offline.
export const PAGES = [
  {
    name: "maison-home",
    url: new URL("./pages/maison-home.html", import.meta.url).href,
    width: 640,
    height: 900,
  },
];
