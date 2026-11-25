const PROVIDERS: Record<string, string> = { google: 'Google', github: 'GitHub', apple: 'Apple', microsoft: 'Microsoft' };

/** The name of the provider an account signs in with, as Clerk calls it ("oauth_google" or "google"); none for email. */
export function providerName(provider: string | undefined): string | undefined {
  if (!provider) return undefined;
  const id = provider.replace(/^oauth_/, '');
  return PROVIDERS[id] ?? id;
}
