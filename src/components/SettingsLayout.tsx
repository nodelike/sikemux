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
