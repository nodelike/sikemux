import type {
  ClerkBackend,
  DemoAccount,
  SignIns,
  SignUp,
} from "../src/account/clerk.ts";

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
    return (
      this.signUps.get(userId) ?? {
        createdAt: new Date(0),
        email: null,
        addresses: [],
      }
    );
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

  readonly userSessions = new Map<string, { id: string; createdAt: Date }[]>();

  async sessions(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return (this.userSessions.get(userId) ?? []).map((session) => ({
      ...session,
      live: !this.revoked.includes(session.id),
    }));
  }

  async appleAccessTokens(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.appleTokens.get(userId) ?? [];
  }

  readonly users = new Map<string, SignIns>();

  async signIns(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.users.get(userId) ?? null;
  }

  async setPrimaryAddress(userId: string, addressId: string) {
    const user = this.users.get(userId);
    if (user) user.primaryAddressId = addressId;
  }

  async removeAddress(addressId: string) {
    for (const user of this.users.values())
      user.addresses = user.addresses.filter((a) => a.id !== addressId);
  }

  async removeExternalAccount(userId: string, accountId: string) {
    const user = this.users.get(userId);
    if (user)
      user.externalAccounts = user.externalAccounts.filter(
        (a) => a.id !== accountId,
      );
  }

  async removePhone(phoneId: string) {
    for (const user of this.users.values())
      user.phones = user.phones.filter((phone) => phone.id !== phoneId);
  }

  async removeWallet(userId: string, walletId: string) {
    const user = this.users.get(userId);
    if (user) user.wallets = user.wallets.filter((w) => w.id !== walletId);
  }

  async replacePassword(userId: string) {
    const user = this.users.get(userId);
    if (user) user.passwordSetAt = new Date(Date.now() + 1);
  }

  /** The users marked as the demo account, and each user's password. */
  readonly demos = new Map<string, DemoAccount>();
  readonly passwords = new Map<string, string>();
  readonly signInTokens: string[] = [];

  async demoAccount(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.demos.get(userId) ?? null;
  }

  async passwordMatches(userId: string, password: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    return this.passwords.get(userId) === password;
  }

  async signInToken(userId: string) {
    if (this.failing) throw new Error("Clerk answered 500");
    const token = `sign-in token ${this.signInTokens.length} for ${userId}`;
    this.signInTokens.push(token);
    return token;
  }
}
