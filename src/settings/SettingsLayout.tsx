import type { ReactNode } from "react";

export function SettingsPage({ children }: { children: ReactNode }) {
    return <div className="settings-page">{children}</div>;
}

export function SettingsSection({ title, meta, sub, children }: { title: ReactNode; meta?: ReactNode; sub?: ReactNode; children: ReactNode }) {
    return (
        <section className="settings-section" data-settings-target={typeof title === "string" ? title : undefined}>
            <header className="settings-section-head">
                <div className="settings-section-topline">
                    <h2 className="settings-section-title">{title}</h2>
                    {meta && <span className="settings-section-meta">{meta}</span>}
                </div>
                {sub && <p className="settings-section-sub">{sub}</p>}
            </header>
            <div className="settings-section-body">{children}</div>
        </section>
    );
}

export function SettingsRows({ children }: { children: ReactNode }) {
    return <div className="settings-rows">{children}</div>;
}

/**
 * The shape every labelled setting takes: a name, an optional line of help, and
 * one control. Pass `asLabel` when the control is a switch or checkbox, so the
 * whole row is clickable; `wide` when the control should fill the right column.
 */
export function SettingsRow({
    label,
    desc,
    wide = false,
    stack = false,
    asLabel = false,
    control,
    children,
}: {
    label: ReactNode;
    desc?: ReactNode;
    wide?: boolean;
    stack?: boolean;
    asLabel?: boolean;
    control?: ReactNode;
    children?: ReactNode;
}) {
    const Tag = asLabel ? "label" : "div";
    return (
        <Tag
            className={`settings-row${wide ? " wide" : ""}${stack ? " stack" : ""}`}
            data-settings-target={typeof label === "string" ? label : undefined}>
            <span className="settings-row-copy">
                <span className="settings-row-label">{label}</span>
                {desc && <span className="settings-row-desc">{desc}</span>}
            </span>
            <span className="settings-row-control">{control ?? children}</span>
        </Tag>
    );
}
