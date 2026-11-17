import { english, type Say } from '@shared/lib/say';

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

type RunningUpdate = { id: string | null; createdAt: Date | null; embedded: boolean };

/**
 * The installed release and the commit of the code running on it: `0.1.0-nightly.5 · cef3bae`, or
 * `0.1.0-nightly.5 · update 1b46d28 (6 Oct)` once an over-the-air update replaced the installed code.
 */
export function versionLabel(version: string | null, commit: string | null, update: RunningUpdate, say: Say = english): string {
  const installed = version ?? say('Unknown version');
  const updated = !update.embedded && Boolean(update.id);
  if (!updated) return commit ? `${installed} · ${commit}` : installed;
  const name = commit ?? (update.id ?? '').replace(/-/g, '').slice(0, 7).toLowerCase();
  const date = update.createdAt ? ` (${update.createdAt.getDate()} ${MONTHS[update.createdAt.getMonth()]})` : '';
  return say('{installed} · update {name}{date}', { installed, name, date });
}
