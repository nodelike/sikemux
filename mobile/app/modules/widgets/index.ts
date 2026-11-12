import { requireOptionalNativeModule } from 'expo';

/** One rolling window of an agent account's plan, in the words the widget shows. */
export type LimitLine = { label: string; used: number; resets: string };

type WidgetsModule = {
  setLimits(lines: LimitLine[]): void;
};

/** The home screen widgets, which show what the app last handed them. */
export const widgets = requireOptionalNativeModule<WidgetsModule>('SikemuxWidgets');
