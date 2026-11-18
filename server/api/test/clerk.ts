import type { ClerkBackend, SignUp } from "../src/account/clerk.ts";

/** Clerk's Backend API as the tests want it to answer: done, or failing until told otherwise. */
export class FakeClerk implements ClerkBackend {
  readonly deleted: string[] = [];
  readonly revoked: string[] = [];
  readonly appleTokens = new Map<string, string[]>();
  readonly signUps = new Map<string, SignUp>();
  failing = false;

  async deleteUser(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    this.deleted.push(userId);
  }

  async profile() {
    if (this.failing) throw new Error("Clerk answered 500");
    return { email: null, name: null, imageUrl: null };
  }

  /** Stands in for an address, so no email-shaped text is in the tests. */
  async verifiedEmail(userId: string): Promise<string | null> {
    if (this.failing) throw new Error("Clerk answered 500");
    return `verified address of ${userId}`;
  }

  async verifiedDomains(): Promise<string[]> {
    if (this.failing) throw new Error("Clerk answered 500");
    return [];
  }

  async signUp(userId: string): Promise<SignUp | null> {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.signUps.get(userId) ?? { createdAt: new Date(0), email: null };
  }

  async people(userIds: string[]) {
    if (this.failing) throw new Error("Clerk answered 500");
    return new Map(
      userIds.map((id) => [id, { name: `Person ${id}`, imageUrl: null }]),
    );
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
