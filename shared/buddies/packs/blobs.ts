import { blobFace, darken, INK, PASTEL, stroke, type BlobFace, type BuddyPack, type BuddyShape } from "../shared";

const S = `stroke="${INK}" stroke-width="1.2" stroke-linejoin="round"`;

function roundStar(cx: number, cy: number, outer: number, inner: number, points = 5): string {
    const pts = Array.from({ length: points * 2 }, (_, i) => {
        const a = -Math.PI / 2 + (i * Math.PI) / points;
        const r = i % 2 ? inner : outer;
        return [cx + Math.cos(a) * r, cy + Math.sin(a) * r];
    });
    const mid = (a: number[], b: number[]) => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    const f = (p: number[]) => p.map((v) => v.toFixed(2)).join(" ");
    let d = "";
    pts.forEach((p, i) => {
        const a = mid(pts[(i - 1 + pts.length) % pts.length], p);
        const b = mid(p, pts[(i + 1) % pts.length]);
        d += `${i ? "L" : "M"}${f(a)}Q${f(p)} ${f(b)}`;
    });
    return d + "Z";
}
const STAR = roundStar(12, 13.4, 11, 6.2);

const blob = (id: string, name: string, body: (c: string) => string, face: BlobFace): BuddyShape => ({
    id,
    name,
    draw: (c, s) => body(c) + blobFace(face, s),
});

export const buddies: BuddyPack = {
    id: "buddies",
    name: "Buddies",
    blurb: "soft pastel blobs",
    colors: PASTEL,
    shapes: [
        blob(
            "mochi",
            "Mochi",
            (c) => `<path d="M2.2 17.4C2.2 10.6 6.6 6.8 12 6.8s9.8 3.8 9.8 10.6c0 3-2.2 4.4-5 4.4H7.2c-2.8 0-5-1.4-5-4.4Z" fill="${c}" ${S}/>`,
            {
                ey: 14.6,
                ex: [8.6, 15.4],
                my: 18.4,
                ck: 17.6,
            },
        ),
        blob("bean", "Bean", (c) => `<rect x="3" y="6.2" width="18" height="16" rx="8" fill="${c}" ${S}/>`, {
            ey: 13.6,
            ex: [8.9, 15.1],
            my: 17.8,
            ck: 16.9,
        }),
        blob(
            "ghostie",
            "Ghostie",
            (c) => `<path d="M4 13.6C4 9.2 7.6 6.4 12 6.4s8 2.8 8 7.2v8.2l-2-1.4-2 1.4-2-1.4-2 1.4-2-1.4-2 1.4-2-1.4-2 1.4Z" fill="${c}" ${S}/>`,
            { ey: 13.4, ex: [9.2, 14.8], my: 16.9, ck: 16.2 },
        ),
        blob(
            "blob",
            "Blob",
            (c) =>
                `<path d="M12 5.6c3.6-.2 7.6 1.6 8.8 5.6 1.2 4.2-.6 8.6-4.6 10.2-3.4 1.4-7.8 1-10.2-1.6-2.6-2.8-2.8-7.4-.6-10.6 1.4-2.2 4-3.5 6.6-3.6Z" fill="${c}" ${S}/>`,
            { ey: 13.2, ex: [9, 15], my: 16.8, ck: 16.2 },
        ),
        blob(
            "cloud",
            "Cloud",
            (c) => `<path d="M6.2 21.6a3.9 3.9 0 0 1-1.4-7.5 5 5 0 0 1 5.6-6.3 4.8 4.8 0 0 1 8.8 2.6 4.6 4.6 0 0 1-1.2 11.2Z" fill="${c}" ${S}/>`,
            {
                ey: 15,
                ex: [9, 15],
                my: 18.6,
                ck: 17.8,
            },
        ),
        blob(
            "heart",
            "Heart",
            (c) =>
                `<path d="M12 21.4C6 17.6 2.4 14.4 2.4 10.6c0-2.8 2.2-4.8 4.8-4.8 2 0 3.6 1 4.8 2.6 1.2-1.6 2.8-2.6 4.8-2.6 2.6 0 4.8 2 4.8 4.8 0 3.8-3.6 7-9.6 10.8Z" fill="${c}" ${S}/>`,
            { ey: 11.8, ex: [8.6, 15.4], my: 15.2, ck: 14.4 },
        ),
        blob("star", "Star", (c) => `<path d="${STAR}" fill="${c}" ${S}/>`, { ey: 13.2, ex: [9.6, 14.4], my: 16.2, ck: 15.6, ckx: 1.8 }),
        blob(
            "peach",
            "Peach",
            (c) =>
                `<path d="M12 8.2C10.6 6.4 8 5.8 6 6.8 3 8.4 2.4 12.4 3.4 15.6 4.6 19.4 8 21.8 12 21.8s7.4-2.4 8.6-6.2c1-3.2.4-7.2-2.6-8.8-2-1-4.6-.4-6 1.4Z" fill="${c}" ${S}/><path d="M12 8.2c-.7 1.6-.8 3.2-.3 4.6" fill="none" stroke="${INK}" stroke-width=".9" stroke-linecap="round" opacity=".45"/>`,
            { ey: 14.6, ex: [8.6, 15.4], my: 18.2, ck: 17.6 },
        ),
        blob(
            "egg",
            "Egg",
            (c) => `<path d="M12 4.4c4.6 0 7.8 5.8 7.8 10.4 0 4.2-3.4 7-7.8 7s-7.8-2.8-7.8-7C4.2 10.2 7.4 4.4 12 4.4Z" fill="${c}" ${S}/>`,
            {
                ey: 14.2,
                ex: [9.2, 14.8],
                my: 17.8,
                ck: 17.1,
            },
        ),
        blob(
            "onigiri",
            "Onigiri",
            (c) =>
                `<path d="M12 4.6c1.5 0 2.5 1 3.4 2.7l5.4 9.2c1.3 2.3 0 5.3-2.9 5.3H6.1c-2.9 0-4.2-3-2.9-5.3l5.4-9.2c.9-1.7 1.9-2.7 3.4-2.7Z" fill="${c}" ${S}/><path d="M9 18.4h6v3.4H9Z" fill="${INK}"/>`,
            { ey: 13.4, ex: [9.6, 14.4], my: 16.2, ck: 15.8, ckx: 2.1 },
        ),
        blob(
            "dumpling",
            "Dumpling",
            (c) =>
                `<path d="M2.4 18.4c0-5.6 4.4-10 9.6-10s9.6 4.4 9.6 10c0 2.2-1.6 3.4-3.6 3.4H6c-2 0-3.6-1.2-3.6-3.4Z" fill="${c}" ${S}/><path d="M9.2 9.2c.6.8 1.6 1 2.8 1s2.2-.2 2.8-1" fill="none" stroke="${INK}" stroke-width="1" stroke-linecap="round"/>`,
            { ey: 15.4, ex: [8.6, 15.4], my: 18.8, ck: 18.2 },
        ),
        blob(
            "pudding",
            "Pudding",
            (c) =>
                `<path d="M4.6 21.6 6.4 10.2c.3-1.9 2.6-3.2 5.6-3.2s5.3 1.3 5.6 3.2l1.8 11.4Z" fill="${c}" ${S}/><path d="M6.4 10.2c.3-1.9 2.6-3.2 5.6-3.2s5.3 1.3 5.6 3.2c-.5 1.3-1.4 1.6-2.2 1.2-.6 1-1.6 1-2.2.3-.8 1-2 1-2.6.1-.7.8-1.7.7-2.2-.2-.9.4-1.8.1-2-1.4Z" fill="${darken(c, 0.35)}" ${S} stroke-width="1"/>`,
            { ey: 15.6, ex: [9.4, 14.6], my: 18.8, ck: 18.2, ckx: 2.1 },
        ),
        blob(
            "toast",
            "Toast",
            (c) =>
                `<path d="M5.6 11.2C3.4 10.8 2.8 8.8 3.6 7.6 4.6 6 7 5.6 12 5.6s7.4.4 8.4 2c.8 1.2.2 3.2-2 3.6V20a2 2 0 0 1-2 2H7.6a2 2 0 0 1-2-2Z" fill="${c}" ${S}/>`,
            { ey: 13.8, ex: [9.2, 14.8], my: 17.6, ck: 16.8 },
        ),
        blob(
            "marshmallow",
            "Marshmallow",
            (c) =>
                `<rect x="3.6" y="6.4" width="16.8" height="15.4" rx="5.4" fill="${c}" ${S}/><path d="M6.6 9.6c3.2 1.4 7.6 1.4 10.8 0" fill="none" stroke="${INK}" stroke-width=".9" stroke-linecap="round" opacity=".4"/>`,
            { ey: 14.6, ex: [9, 15], my: 18.2, ck: 17.5 },
        ),
        blob(
            "penguin",
            "Penguin",
            (c) =>
                `<ellipse cx="3.6" cy="16" rx="1.4" ry="2.6" transform="rotate(20 3.6 16)" fill="${darken(c, 0.2)}" ${S}/><ellipse cx="20.4" cy="16" rx="1.4" ry="2.6" transform="rotate(-20 20.4 16)" fill="${darken(c, 0.2)}" ${S}/><path d="M12 5c4.6 0 8 4 8 9.2 0 4.6-3.4 7.6-8 7.6s-8-3-8-7.6C4 9 7.4 5 12 5Z" fill="${c}" ${S}/><path d="M12 10.2c3 0 5.4 2 5.4 5 0 3.2-2.4 5-5.4 5s-5.4-1.8-5.4-5c0-3 2.4-5 5.4-5Z" fill="#fff" opacity=".85"/>`,
            { ey: 14.4, ex: [9.6, 14.4], my: 17.2, ck: 17, ckx: 1.6 },
        ),
        blob(
            "slime",
            "Slime",
            (c) => `<path d="M12 5.8c2.8 3.2 8.6 6.8 8.6 11.6 0 3-2.4 4.6-5 4.6H8.4c-2.6 0-5-1.6-5-4.6 0-4.8 5.8-8.4 8.6-11.6Z" fill="${c}" ${S}/>`,
            { ey: 15.4, ex: [9, 15], my: 19, ck: 18.2 },
        ),
    ],
};

const BODY = `x="3.4" y="5" width="17.2" height="17" rx="7.6"`;
const PERSONA_FACE: BlobFace = { ey: 12.4, ex: [9.2, 14.8], my: 16, ck: 15.3, ckx: 2.4 };
const hand = (x: number, y: number, c: string) => `<circle cx="${x}" cy="${y}" r="1.25" fill="${darken(c, 0.06)}" ${stroke(0.8)}/>`;
const inside = (uid: string, inner: string) =>
    `<clipPath id="${uid}k"><rect ${BODY}/></clipPath><g clip-path="url(#${uid}k)">${inner}</g><rect ${BODY} fill="none" ${stroke(1.2)}/>`;

interface Role {
    id: string;
    name: string;
    back?: (c: string) => string;
    front: (c: string, uid: string) => string;
}

const ROLES: Role[] = [
    {
        id: "ceo",
        name: "CEO",
        front: (_c, uid) =>
            inside(
                uid,
                `<rect x="0" y="18.2" width="24" height="6" fill="#2c2836"/><path d="M9.2 18.2 12 21.8l2.8-3.6Z" fill="#fff"/><path d="M11.3 18.5h1.4l-.3.9h-.8ZM11.6 19.4h.8l.6 2.4-1 1-1-1Z" fill="#e2513a"/>`,
            ),
    },
    {
        id: "cto",
        name: "CTO",
        back: () => `<rect x="1.8" y="3.4" width="20.4" height="19.8" rx="9" fill="#3b3546" ${stroke(1.1)}/>`,
        front: () =>
            `<path d="M9.8 18.6v2.6M14.2 18.6v2.6" stroke="#e7e5ef" stroke-width=".8" stroke-linecap="round"/><circle cx="9.8" cy="21.5" r=".5" fill="#e7e5ef"/><circle cx="14.2" cy="21.5" r=".5" fill="#e7e5ef"/>`,
    },
    {
        id: "cpo",
        name: "CPO",
        front: (c) =>
            `<rect x="15.6" y="12.6" width="6.8" height="8.8" rx=".9" fill="#c08a4a" ${stroke(0.9)}/><rect x="16.4" y="14" width="5.2" height="6.8" fill="#fff"/><rect x="17.6" y="12" width="2.8" height="1.4" rx=".4" fill="#8a8a96" ${stroke(0.6)}/><rect x="17" y="15.2" width="3" height=".9" rx=".4" fill="#7cd992"/><rect x="18" y="16.8" width="3" height=".9" rx=".4" fill="#ffc94a"/><rect x="17.4" y="18.4" width="2.4" height=".9" rx=".4" fill="#ff7aa8"/>${hand(15.8, 18.6, c)}`,
    },
    {
        id: "scientist",
        name: "Chief Scientist",
        front: (_c, uid) =>
            inside(
                uid,
                `<rect x="0" y="18.6" width="24" height="6" fill="#f4f2f8"/><path d="M12 18.6v4M9.6 18.6l2.4 2.4 2.4-2.4" stroke="#b8b4c6" stroke-width=".7" fill="none"/>`,
            ) +
            `<path d="M3.4 12.4h17.2" stroke="#5a5068" stroke-width="1.1"/>` +
            PERSONA_FACE.ex
                .map(
                    (x) =>
                        `<circle cx="${x}" cy="${PERSONA_FACE.ey}" r="2.7" fill="#9fe8ff" fill-opacity=".3" stroke="#5a5068" stroke-width="1.1"/><path d="M${x - 1.6} ${PERSONA_FACE.ey - 1}q.6-1 1.6-1.3" stroke="#fff" stroke-width=".5" fill="none" opacity=".8"/>`,
                )
                .join(""),
    },
    {
        id: "designer",
        name: "Designer",
        front: () =>
            `<path d="M2.4 17.4c0-2.2 1.8-3.8 4-3.8 2 0 3.4 1.2 3.4 2.6 0 1-1 1.2-1 2 0 .9 1.2 1.2 1.2 2 0 1-1.5 1.9-3.4 1.9-2.4 0-4.2-1.9-4.2-4.7Z" fill="#e8c99a" ${stroke(0.9)}/><circle cx="4.4" cy="16.4" r=".75" fill="#ff5f8f"/><circle cx="6.4" cy="15.4" r=".75" fill="#3ba7ff"/><circle cx="4.2" cy="19" r=".75" fill="#ffc83a"/><circle cx="6.2" cy="20.4" r=".75" fill="#2ee07f"/><circle cx="8" cy="17.8" r=".55" fill="#2c2836"/><circle cx="18.2" cy="15.2" r=".7" fill="#3ba7ff" opacity=".8"/>`,
    },
    {
        id: "engineer",
        name: "Engineer",
        front: () =>
            `<rect x="6" y="16.8" width="12" height="4.6" rx=".8" fill="#a9a6b6" ${stroke(0.9)}/><path d="M10.6 18.3l-1 .8 1 .8M13.4 18.3l1 .8-1 .8M12.4 17.9l-.8 2.4" stroke="#7cf0ff" stroke-width=".7" fill="none" stroke-linecap="round"/><path d="M4.6 21.2h14.8l1.2 1.6H3.4Z" fill="#c9c6d4" ${stroke(0.8)}/>`,
    },
    {
        id: "devops",
        name: "DevOps",
        front: (c) =>
            `<path d="M16.4 21.2l3.6-4.6" stroke="#9aa0ad" stroke-width="2" stroke-linecap="round"/><circle cx="20.4" cy="15.8" r="1.9" fill="#9aa0ad" ${stroke(0.8)}/><path d="M20 14.2l1.3-.5.4 1.4-1.1.5Z" fill="${darken(c, 0.05)}"/>${hand(17.2, 20, c)}<path d="M9.8 19.8c0-.9 1.2-.9 2.2 0s2.2.9 2.2 0-1.2-.9-2.2 0-2.2.9-2.2 0Z" stroke="#3d5a9e" stroke-width="1" fill="none"/>`,
    },
    {
        id: "publisher",
        name: "Publisher",
        front: () =>
            `<rect x="4.4" y="16.2" width="15.2" height="6.6" rx=".4" fill="#f4f0e6" ${stroke(0.9)}/><rect x="5.4" y="17" width="8.4" height="1.3" fill="#2c2836"/><path d="M5.4 19.4h5.4M5.4 20.6h5.4M5.4 21.8h4" stroke="#9a96a6" stroke-width=".5"/><rect x="11.6" y="19" width="7" height="3.2" fill="#c7a9ff" opacity=".7"/><path d="M7 13.8q2.2 1 4.2 0M12.8 13.8q2.2 1 4.2 0" stroke="${INK}" stroke-width=".7" fill="none"/>`,
    },
    {
        id: "marketer",
        name: "Marketer",
        front: (c) =>
            `<path d="M8 14.6 2.4 11.6v7.6l5.6-2.8Z" fill="#ff6b5c" ${stroke(0.9)}/><rect x="7.6" y="14" width="1.8" height="3" rx=".6" fill="#f4f2f8" ${stroke(0.7)}/>${hand(8.8, 17.4, c)}`,
    },
    {
        id: "sales",
        name: "Sales",
        front: (c) =>
            `<rect x="17.6" y="8.8" width="3.2" height="6.2" rx=".9" transform="rotate(14 19.2 12)" fill="#2c2836" ${stroke(0.8)}/>${hand(18.8, 15.2, c)}<circle cx="12" cy="19.8" r="1.5" fill="#ffd23a" ${stroke(0.7)}/><path d="M12.6 19.1c-.3-.3-1.5-.4-1.5.3s1.6.4 1.6 1.1-1.3.6-1.6.3M12 18.6v.4M12 20.8v.4" stroke="${INK}" stroke-width=".4" fill="none"/>`,
    },
    {
        id: "data",
        name: "Data",
        front: (c) =>
            `<rect x="15.2" y="12" width="7" height="8.6" rx=".9" fill="#fff" ${stroke(0.9)}/><rect x="16.3" y="17" width="1.3" height="2.6" fill="#3ba7ff"/><rect x="18.1" y="15.4" width="1.3" height="4.2" fill="#9a62ff"/><rect x="19.9" y="13.4" width="1.3" height="6.2" fill="#2ee07f"/>${hand(15.4, 18.4, c)}`,
    },
    {
        id: "qa",
        name: "QA",
        front: (c) =>
            `<path d="M17 14.6l3.4 3.4" stroke="#8a6a4a" stroke-width="1.8" stroke-linecap="round"/><circle cx="14.8" cy="12.4" r="3.1" fill="#bfe9ff" fill-opacity=".28" stroke="#8a6a4a" stroke-width="1.2"/><path d="M13 11q.6-1 1.8-1.2" stroke="#fff" stroke-width=".5" fill="none"/>${hand(19.8, 17.8, c)}`,
    },
    {
        id: "security",
        name: "Security",
        front: () =>
            `<path d="M5.8 11.4h6.8v1.6c0 1.2-.8 1.8-1.8 1.8H7.6c-1 0-1.8-.6-1.8-1.8ZM11.4 11.4h6.8v1.6c0 1.2-.8 1.8-1.8 1.8h-3.2c-1 0-1.8-.6-1.8-1.8Z" fill="${INK}" fill-opacity=".82" ${stroke(0.7)}/><path d="M12 19.2l1.8.7v1.2c0 1-.8 1.6-1.8 2-1-.4-1.8-1-1.8-2v-1.2Z" fill="#3d5a9e" ${stroke(0.6)}/><path d="M11.3 20.8l.5.5 1-1" stroke="#fff" stroke-width=".5" fill="none"/>`,
    },
    {
        id: "support",
        name: "Support",
        front: () =>
            `<rect x="2.4" y="10.6" width="2.6" height="4.2" rx="1.2" fill="#2c2836" ${stroke(0.8)}/><path d="M3.8 14.6q.4 3 5.2 2.8" stroke="#2c2836" stroke-width=".9" fill="none" stroke-linecap="round"/><circle cx="9.4" cy="17.4" r=".8" fill="#2c2836"/>`,
    },
    {
        id: "cfo",
        name: "CFO",
        front: (c) =>
            `<circle cx="14.8" cy="12.4" r="2.6" fill="none" stroke="#d9a43a" stroke-width="1"/><path d="M17.2 13.4q1.6 3 .6 6.4" stroke="#d9a43a" stroke-width=".5" fill="none"/>${[21.2, 19.8, 18.4].map((y) => `<ellipse cx="4.4" cy="${y}" rx="2.2" ry=".85" fill="#f2c14e" ${stroke(0.6)}/>`).join("")}${hand(6.8, 19.2, c)}`,
    },
    {
        id: "intern",
        name: "Intern",
        front: (c) =>
            `<path d="M16.2 14.6h4.6l-.6 6.6c0 .5-.4.8-.9.8h-1.8c-.5 0-.9-.3-.9-.8Z" fill="#fff" ${stroke(0.8)}/><rect x="16.4" y="16.8" width="4.2" height="2.2" fill="#c08a4a"/><rect x="15.8" y="13.6" width="5.4" height="1.2" rx=".5" fill="#f4f2f8" ${stroke(0.7)}/>${hand(16.2, 18.6, c)}<path d="M9.6 17.6l2.4 2.2 2.4-2.2" stroke="#3ba7ff" stroke-width=".7" fill="none"/><rect x="10.6" y="19.6" width="2.8" height="2.2" rx=".3" fill="#fff" ${stroke(0.6)}/>`,
    },
];

export const personas: BuddyPack = {
    id: "personas",
    name: "Personas",
    blurb: "startup roles",
    colors: PASTEL,
    shapes: ROLES.map((role) => ({
        id: role.id,
        name: role.name,
        draw: (c, s, uid) => `${role.back?.(c) ?? ""}<rect ${BODY} fill="${c}" ${stroke(1.2)}/>${blobFace(PERSONA_FACE, s)}${role.front(c, uid)}`,
    })),
};
