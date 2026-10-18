import { compareVersions } from '@/network/versions';

/** The first Sikemux release whose host answers each call the phone makes that older hosts do not know. */
const FIRST_HOST_WITH = {
  questions: '0.5.3-nightly.2',
  answerQuestion: '0.5.3-nightly.2',
} as const;

export type NewerCall = keyof typeof FIRST_HOST_WITH;

/** A host whose version does not read as a release, such as a local build, is taken to know every call. */
export function hostKnows(version: string, call: NewerCall): boolean {
  const order = compareVersions(version, FIRST_HOST_WITH[call]);
  return order === null || order >= 0;
}
