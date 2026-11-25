import type { Org, OrgMember } from "@sikemux/protocol";
import { IconLink } from "@shared/ui/Icons";
import type { MouseEvent } from "react";

import { PersonAvatar } from "./Avatar.tsx";
import "./members.css";
import { frozen } from "./orgs.ts";
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

export const nameOf = (member: OrgMember) => member.name ?? "Someone";

export const people = (count: number) =>
  `${count} ${count === 1 ? "person" : "people"}`;

export const freeSeats = ({ seats }: Org) =>
  Math.max(0, seats.limit - seats.held);

export const copyText = (text: string) =>
  navigator.clipboard.writeText(text).then(
    () => true,
    () => false,
  );

/** Compact seats bar shown in Members' tools strip. */
export function SeatsStrip({
  org,
  href,
  onClick,
}: {
  org: Org;
  href?: string;
  onClick?: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { held, limit } = org.seats;
  const pct = limit > 0 ? Math.min(100, (held / limit) * 100) : 100;
  const label = frozen(org)
    ? `${held} ${held === 1 ? "seat" : "seats"} taken`
    : `${held} of ${limit} seats used`;
  const inner = (
    <>
      <span className="m-bar" aria-hidden="true">
        <i style={{ width: `${pct}%` }} />
      </span>
      <span>{label}</span>
    </>
  );
  if (href && onClick)
    return (
      <a className="m-seats" href={href} onClick={onClick}>
        {inner}
      </a>
    );
  return <span className="m-seats">{inner}</span>;
}

/** Who holds an org's seats: people by their picture, links out and free ones as circles. */
export function Seats({ org, members }: { org: Org; members: OrgMember[] }) {
  const free = freeSeats(org);
  const links = Math.max(0, org.seats.held - members.length);
  return (
    <Section label="Seats">
      <div className="m-seatw">
        <div className="m-seatstrip" aria-hidden="true">
          {members.map((member) => (
            <MemberAvatar key={member.id} member={member} size={30} />
          ))}
          {Array.from({ length: links }, (_, at) => (
            <span className="m-seat-link" key={`link${at}`}>
              <IconLink size={13} />
            </span>
          ))}
          {Array.from({ length: Math.min(free, 200) }, (_, at) => (
            <span className="m-seat-free" key={`free${at}`} />
          ))}
        </div>
        <p className="note">
          {people(members.length)}, {links} {links === 1 ? "link" : "links"} out
          {frozen(org) ? "" : `, ${free} free`}
        </p>
      </div>
    </Section>
  );
}
