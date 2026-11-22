import * as Crypto from 'expo-crypto';

import { api, ApiProblem } from './api';

const signedOut = api();

/** The demo account's address hashed, or null when the accounts API has none; asked once per launch. */
let demoAddress: Promise<string | null> | undefined;

const askDemoAddress = () =>
  signedOut.getReviewAccount().then(
    (account) => account.addressSha256,
    (error: unknown) => {
      if (error instanceof ApiProblem && error.status === 404) return null;
      demoAddress = undefined;
      return null;
    },
  );

/**
 * The store reviewers' demo account. The API publishes only its address's hash, so the phone tells
 * it apart here and never sends another account's password to the API.
 */
export const review = {
  async isDemo(email: string): Promise<boolean> {
    const demo = await (demoAddress ??= askDemoAddress());
    if (!demo) return false;
    const typed = await Crypto.digestStringAsync(Crypto.CryptoDigestAlgorithm.SHA256, email.trim().toLowerCase());
    return typed === demo;
  },
  async ticket(email: string, password: string): Promise<string> {
    return (await signedOut.reviewSignIn({ email: email.trim(), password })).ticket;
  },
};
