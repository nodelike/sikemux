import { useUser } from "@clerk/react";
import { IconMail, IconTrash } from "@shared/ui/Icons";
import type { MouseEvent } from "react";

import { Avatar } from "./Avatar.tsx";
import { DELETE_ACCOUNT } from "./navigation.ts";
import { PROVIDERS, SignInWaysSection } from "./SignInWays.tsx";
import { ItemRow, PageHead, Row, Rows, Section } from "./ui.tsx";
import { WorkAddressesSection } from "./WorkAddresses.tsx";

/** The ways the signed-in account can sign in, as the person knows them. */
export function useSignInWays() {
  const { user } = useUser();
  const providers = (user?.externalAccounts ?? []).flatMap((account) => {
    const provider = PROVIDERS[account.provider];
    return provider ? [provider] : [];
  });
  return providers.length > 0
    ? providers
    : [{ name: "Email code", mark: <IconMail size={16} /> }];
}

export function AccountPage({
  onDelete,
}: {
  onDelete: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { user } = useUser();
  const email = user?.primaryEmailAddress?.emailAddress;

  return (
    <>
      <PageHead title="Account">
        Who you are on Sikemux, and the way out.
      </PageHead>
      <div className="groups">
        <Section label="Profile">
          <Rows>
            <Row label="Name">
              <Avatar size={28} />
              <span>{user?.fullName || user?.username || "No name yet"}</span>
            </Row>
            {email ? <Row label="Email">{email}</Row> : null}
          </Rows>
        </Section>
        <SignInWaysSection />
        <WorkAddressesSection />
        <Section label="Delete">
          <Rows>
            <ItemRow
              glyph={<IconTrash size={16} />}
              name="Delete account"
              detail="Every host and client is signed out, and phones are removed from every host."
              end={
                <a
                  className="button small quiet-danger"
                  href={DELETE_ACCOUNT}
                  onClick={onDelete}
                >
                  Delete account…
                </a>
              }
            />
          </Rows>
        </Section>
      </div>
    </>
  );
}
