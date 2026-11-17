import { english, type Say } from '@shared/lib/say';

/** The app speaks English only until someone reviews a translation; its words still go through `useT`. */
export type Language = 'en';
export type Translate = Say;

/** Words said outside a screen's drawing, such as an alert or a notification. */
export const translate: Translate = english;

export function useLanguage(): Language {
  return 'en';
}

export function useT(): Translate {
  return english;
}
