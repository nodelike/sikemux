import type { Org, OrgMember } from "@sikemux/protocol";
import type { CSSProperties } from "react";

import { PersonAvatar } from "./Avatar.tsx";
import { frozen } from "./orgs.ts";
import { stippledDisc } from "./stipple.ts";
import { Section } from "./ui.tsx";

export function MemberAvatar({
  member,
  size,
}: {
  member: OrgMember;
  size?: number;
}) {
  return (
    <PersonAvatar
      seed={member.avatarSeed}
      imageUrl={member.imageUrl}
      size={size}
    />
  );
}

export const people = (count: number) =>
  `${count} ${count === 1 ? "person" : "people"}`;

export const freeSeats = ({ seats }: Org) =>
  Math.max(0, seats.limit - seats.held);

/** Who holds an org's seats: people by their picture, links out and free ones as dots. */
export function Seats({ org, members }: { org: Org; members: OrgMember[] }) {
  const free = freeSeats(org);
  const links = Math.max(0, org.seats.held - members.length);
  const seats = { "--seat-mask": stippledDisc() } as CSSProperties;
  return (
    <Section label="Seats">
      <div className="seatw">
        <div>
          <div className="big">
            {org.seats.held}
            <small>{frozen(org) ? "taken" : `of ${org.seats.limit}`}</small>
          </div>
          <p className="note">
            {people(members.length)}, {links} {links === 1 ? "link" : "links"}{" "}
            out{frozen(org) ? "" : `, ${free} free`}
          </p>
        </div>
        <div className="seats" style={seats} aria-hidden="true">
          {members.map((member) => (
            <span className="s" key={member.id}>
              <MemberAvatar member={member} />
            </span>
          ))}
          {Array.from({ length: links }, (_, at) => (
            <span className="s h" key={`link${at}`} />
          ))}
          {Array.from({ length: Math.min(free, 200) }, (_, at) => (
            <span className="s f" key={`free${at}`} />
          ))}
        </div>
      </div>
    </Section>
  );
}
