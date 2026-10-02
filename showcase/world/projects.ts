export const DEMO_HOME = "/Users/edon";

export const PANE_IMAGE = `${DEMO_HOME}/Pictures/jinx-graffiti.jpg`;

export const DEMO_PROJECTS = [
  { name: "sikemux", path: `${DEMO_HOME}/code/sikemux` },
  { name: "sikemux-front", path: `${DEMO_HOME}/code/sikemux-front` },
  { name: "moodboard-studio", path: `${DEMO_HOME}/code/moodboard-studio` },
] as const;

export const [SIKEMUX, FRONT, MOODBOARD] = DEMO_PROJECTS.map(
  (project) => project.path,
);
