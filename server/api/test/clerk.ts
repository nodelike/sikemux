import type { ClerkBackend } from "../src/account/clerk.ts";

/** Clerk's Backend API as the tests want it to answer: done, or failing until told otherwise. */
export class FakeClerk implements ClerkBackend {
  readonly deleted: string[] = [];
  readonly revoked: string[] = [];
  readonly appleTokens = new Map<string, string[]>();
  failing = false;

  async deleteUser(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    this.deleted.push(userId);
  }

  async profile() {
    if (this.failing) throw new Error("Clerk answered 500");
    return { email: null, name: null, imageUrl: null };
  }

  async revokeSession(sessionId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    this.revoked.push(sessionId);
  }

  async appleAccessTokens(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.appleTokens.get(userId) ?? [];
  }
}
