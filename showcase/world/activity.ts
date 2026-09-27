import type {
  ActivityDay,
  ActivityShare,
  ActivitySummary,
} from "../../src/api/activity";
import { dayDate, localDay, weekday } from "../../src/lib/activityCalendar";
import { FRONT, MOODBOARD, SIKEMUX } from "./projects";

const HOUR = 3_600_000;
const DAYS_OF_HISTORY = 350;
const RAMP_DAYS = 120;

function seeded(seed: number): () => number {
  let state = seed;
  return () => {
    state = (state * 1_103_515_245 + 12_345) % 2_147_483_648;
    return state / 2_147_483_648;
  };
}

function demoDays(today: number): ActivityDay[] {
  const random = seeded(26);
  const first = today - DAYS_OF_HISTORY;
  const days: ActivityDay[] = [];
  for (let day = first; day <= today; day += 1) {
    const weekend = weekday(day) >= 5;
    const rest = random() < (weekend ? 0.5 : 0.07);
    const swing = 0.45 + random() * 1.1;
    if (rest) continue;
    const ramp = Math.min(1, 0.15 + (day - first) / RAMP_DAYS);
    const load = ramp * swing * (weekend ? 0.45 : 1);
    days.push({
      day,
      sessions: Math.max(1, Math.round(7 * load)),
      agentMs: Math.round(3.4 * HOUR * load),
      commits: Math.round(11 * load * (0.4 + random() * 0.9)),
      tokens: Math.round(46_000_000 * load),
    });
  }
  return days;
}

function share(
  name: string,
  part: number,
  totals: Omit<ActivityShare, "name">,
): ActivityShare {
  return {
    name,
    sessions: Math.round(totals.sessions * part),
    agentMs: Math.round(totals.agentMs * part),
    commits: Math.round(totals.commits * part),
    tokens: Math.round(totals.tokens * part),
  };
}

export function demoActivity(): ActivitySummary {
  const days = demoDays(localDay());
  const sum = (field: keyof Omit<ActivityDay, "day">) =>
    days.reduce((total, day) => total + day[field], 0);
  const whole = {
    sessions: sum("sessions"),
    agentMs: sum("agentMs"),
    commits: sum("commits"),
    tokens: sum("tokens"),
  };
  return {
    totals: {
      sessions: whole.sessions,
      resumed: Math.round(whole.sessions * 0.31),
      turns: whole.sessions * 9,
      agentMs: whole.agentMs,
      commits: whole.commits,
      agentCommits: Math.round(whole.commits * 0.72),
      input: Math.round(whole.tokens * 0.04),
      output: Math.round(whole.tokens * 0.02),
      cacheRead: Math.round(whole.tokens * 0.87),
      cacheWrite: Math.round(whole.tokens * 0.07),
      firstAtMs: dayDate(days[0].day).getTime(),
    },
    days,
    agents: [
      share("claude", 0.54, whole),
      share("codex", 0.35, whole),
      share("hermes", 0.11, whole),
    ],
    projects: [
      share(SIKEMUX, 0.61, whole),
      share(MOODBOARD, 0.24, whole),
      share(FRONT, 0.15, whole),
    ],
  };
}
