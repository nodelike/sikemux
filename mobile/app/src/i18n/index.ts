import { AppState } from 'react-native';
import { getLocales, type Locale } from 'expo-localization';
import * as SecureStore from 'expo-secure-store';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';
import { english, type Say } from '@shared/lib/say';
import de from './de.json';
import es from './es.json';
import fr from './fr.json';
import ja from './ja.json';
import ko from './ko.json';
import ptBR from './pt-BR.json';
import ru from './ru.json';
import zhHans from './zh-Hans.json';

/** Each language by its own name, in the order the picker lists them. English is the source, so it has no table. */
export const LANGUAGES = [
  { code: 'en', name: 'English' },
  { code: 'es', name: 'Español' },
  { code: 'zh-Hans', name: '简体中文' },
  { code: 'de', name: 'Deutsch' },
  { code: 'fr', name: 'Français' },
  { code: 'ja', name: '日本語' },
  { code: 'ko', name: '한국어' },
  { code: 'ru', name: 'Русский' },
  { code: 'pt-BR', name: 'Português (Brasil)' },
] as const;

export type Language = (typeof LANGUAGES)[number]['code'];
export type LanguageChoice = Language | 'system';
export type Translate = Say;

const TABLES: Record<Exclude<Language, 'en'>, Record<string, string>> = {
  es,
  'zh-Hans': zhHans,
  de,
  fr,
  ja,
  ko,
  ru,
  'pt-BR': ptBR,
};

const ITEM = 'sikemux.language';

function stored(): LanguageChoice {
  try {
    const saved = SecureStore.getItem(ITEM);
    return LANGUAGES.some((language) => language.code === saved) ? (saved as Language) : 'system';
  } catch {
    return 'system';
  }
}

function supported({ languageCode, languageScriptCode, regionCode }: Locale): Language | undefined {
  switch (languageCode) {
    case 'zh':
      return languageScriptCode === 'Hant' || (!languageScriptCode && ['TW', 'HK', 'MO'].includes(regionCode ?? ''))
        ? undefined
        : 'zh-Hans';
    case 'pt':
      return 'pt-BR';
    case 'en':
    case 'es':
    case 'de':
    case 'fr':
    case 'ja':
    case 'ko':
    case 'ru':
      return languageCode;
    default:
      return undefined;
  }
}

function systemLanguage(): Language {
  try {
    for (const locale of getLocales()) {
      const language = supported(locale);
      if (language) return language;
    }
  } catch {
    // A platform that can't say its languages reads in English.
  }
  return 'en';
}

const choice = externalValue<LanguageChoice>(stored());
const system = externalValue<Language>(systemLanguage(), () => {
  const following = AppState.addEventListener('change', (state) => {
    if (state === 'active') system.set(systemLanguage());
  });
  return () => following.remove();
});

const translators = new Map<Exclude<Language, 'en'>, Translate>();

function translatorFor(language: Language): Translate {
  if (language === 'en') return english;
  let translate = translators.get(language);
  if (!translate) {
    const table = TABLES[language];
    translate = (text, values) => english(table[text] || text, values);
    translators.set(language, translate);
  }
  return translate;
}

const currentLanguage = () => (choice.get() === 'system' ? system.get() : (choice.get() as Language));

/** Words said outside a screen's drawing, such as an alert or a notification, in the language the app speaks now. */
export const translate: Translate = (text, values) => translatorFor(currentLanguage())(text, values);

export function useLanguageChoice(): LanguageChoice {
  return useExternalValue(choice);
}

/** The language the app speaks: the one the person chose, or the system's. */
export function useLanguage(): Language {
  const chosen = useExternalValue(choice);
  const fromSystem = useExternalValue(system);
  return chosen === 'system' ? fromSystem : chosen;
}

/** The words of the language the app speaks; a new function each time it changes, so whatever kept the old words draws again. */
export function useT(): Translate {
  return translatorFor(useLanguage());
}

export async function chooseLanguage(next: LanguageChoice) {
  choice.set(next);
  if (next === 'system') await SecureStore.deleteItemAsync(ITEM);
  else await SecureStore.setItemAsync(ITEM, next);
}
