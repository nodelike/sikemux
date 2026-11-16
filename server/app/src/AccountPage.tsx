import { useUser } from "@clerk/react";
import { IconMail, IconTrash } from "@shared/ui/Icons";
import type { MouseEvent, ReactNode } from "react";

import { Avatar } from "./Avatar.tsx";
import { AppleMark, GitHubMark, GoogleMark } from "./icons.tsx";
import { DELETE_ACCOUNT } from "./navigation.ts";
import { ItemRow, PageHead, Row, Rows, Section } from "./ui.tsx";

const PROVIDERS: Record<string, { name: string; mark: ReactNode }> = {
  google: { name: "Google", mark: <GoogleMark /> },
  apple: { name: "Apple", mark: <AppleMark /> },
  github: { name: "GitHub", mark: <GitHubMark /> },
};

export function AccountPage({
  onDelete,
}: {
  onDelete: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { user } = useUser();
  const providers = (user?.externalAccounts ?? []).flatMap((account) => {
    const provider = PROVIDERS[account.provider];
    return provider ? [provider] : [];
  });
  const ways =
    providers.length > 0
      ? providers
      : [{ name: "Email code", mark: <IconMail size={16} /> }];

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
            <Row label="Signs in with">
              {ways.map((way, index) => (
                <span className="way" key={way.name}>
                  {index > 0 ? <span className="quiet">·</span> : null}
                  {way.mark}
                  <span>{way.name}</span>
                </span>
              ))}
            </Row>
          </Rows>
        </Section>
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
