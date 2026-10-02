import { Fragment, type CSSProperties, type ReactNode } from "react";

/**
 * The app's panel vocabulary.
 *
 * Every pane used to hand-roll the same four things — a labelled header with
 * actions, a scrolling body, selectable rows, and an empty state — under its
 * own class names (`git-panel-*`, `rail-group-*`, `rnd-section-*`,
 * `bruno-section-*`). These are the shared versions; feature CSS should only
 * describe what genuinely differs.
 */

/**
 * Height of a `PanelRow`, in px.
 *
 * Virtualised lists position rows absolutely from this number, so it has to
 * match `.panel-row { height }` in panel.css exactly — a drift makes rows
 * overlap once a list is long enough to virtualise.
 */
export const PANEL_ROW_HEIGHT = 24;

/**
 * How a stacking panel claims height.
 *
 * A content-sized panel must not shrink: with `flex-shrink: 1` a long list
 * elsewhere in the column squeezes it down to its min-height and clips its
 * rows. It is capped at 45% instead, and scrolls its own body past that. The
 * grower is the mirror image — it must be allowed *below* its content height,
 * or it pushes the fixed panels out of the column entirely.
 */
function blockSizing(flex: number): CSSProperties {
    return flex === 0 ? { flex: "0 0 auto", maxHeight: "45%" } : { flex: `${flex} 1 auto`, minHeight: 0 };
}

export interface PanelAction {
    /** Single-key shortcut shown as a chip on the action. */
    key?: string;
    label: string;
    onClick: () => void;
    tone?: "warn" | "danger";
    disabled?: boolean;
}

/**
 * A panel surface.
 *
 * `variant="block"` is the bordered, focusable, flex-sized panel used when
 * several stack in one pane. `variant="group"` is the lighter rail grouping —
 * a labelled run of rows with no border of its own.
 */
export function Panel({
    variant = "block",
    focused = false,
    flex,
    className,
    children,
}: {
    variant?: "block" | "group";
    focused?: boolean;
    /**
     * `0` sizes the panel to its content; `1` makes it the one that absorbs
     * whatever height is left over.
     *
     * Sharing the height by weight meant five branches still claimed a third
     * of the pane, because every panel had to grow into the space. Only one
     * panel should, and the rest should ask for exactly what they need.
     */
    flex?: number;
    className?: string;
    children: ReactNode;
}) {
    return (
        <div
            className={`panel panel-${variant}${focused ? " focused" : ""}${className ? ` ${className}` : ""}`}
            style={variant === "block" && flex !== undefined ? blockSizing(flex) : undefined}>
            {children}
        </div>
    );
}

/**
 * A panel's header row: optional ordinal, an eyebrow label, badges, and
 * actions that reveal on hover or focus.
 */
export function PanelHeader({
    index,
    label,
    badges,
    actions,
    extra,
    onFocus,
    rule = false,
}: {
    index?: number;
    label: string;
    /** Rendered after the label; falsy entries are skipped. Pass `<Badge>`s. */
    badges?: ReactNode[];
    actions?: PanelAction[];
    /** Arbitrary content between the badges and the actions. */
    extra?: ReactNode;
    /** Makes the header itself a focus target for the panel it labels. */
    onFocus?: () => void;
    /** Draws a hairline from the label to the actions (the rail grouping look). */
    rule?: boolean;
}) {
    const visibleBadges = (badges ?? []).filter(Boolean);
    return (
        <div
            className="panel-head"
            role={onFocus ? "button" : undefined}
            tabIndex={onFocus ? 0 : undefined}
            aria-label={onFocus ? `Focus ${label} panel` : undefined}
            onClick={onFocus}
            onKeyDown={
                onFocus
                    ? (event) => {
                          if (event.key !== "Enter" && event.key !== " ") return;
                          event.preventDefault();
                          onFocus();
                      }
                    : undefined
            }>
            {index !== undefined && <span className="panel-index">{index}</span>}
            <span className="panel-label">{label}</span>
            {visibleBadges.map((badge, i) => (
                <Fragment key={i}>{badge}</Fragment>
            ))}
            {rule && <span className="panel-rule" />}
            {extra}
            {actions && actions.length > 0 && (
                <span className="panel-actions">
                    {actions.map((action) => (
                        <button
                            key={action.label}
                            type="button"
                            className={`panel-abtn${action.tone ? ` ${action.tone}` : ""}`}
                            disabled={action.disabled}
                            onClick={(event) => {
                                event.stopPropagation();
                                action.onClick();
                            }}>
                            {action.label}
                            {action.key && <kbd className="panel-akbd">{action.key}</kbd>}
                        </button>
                    ))}
                </span>
            )}
        </div>
    );
}

/** The scrolling region under a header. */
export function PanelBody({ className, children }: { className?: string; children: ReactNode }) {
    return <div className={`panel-body${className ? ` ${className}` : ""}`}>{children}</div>;
}

/**
 * A selectable list row. `ranged` marks membership of a multi-row selection,
 * which reads differently from the cursor position.
 */
export function PanelRow({
    selected = false,
    ranged = false,
    muted = false,
    onClick,
    className,
    children,
}: {
    selected?: boolean;
    ranged?: boolean;
    muted?: boolean;
    onClick?: () => void;
    className?: string;
    children: ReactNode;
}) {
    const state = `${selected ? " sel" : ""}${ranged ? " ranged" : ""}${muted ? " muted" : ""}`;
    return (
        <div
            className={`panel-row${state}${className ? ` ${className}` : ""}`}
            role={onClick ? "button" : undefined}
            tabIndex={onClick ? 0 : undefined}
            onClick={onClick}
            onKeyDown={
                onClick
                    ? (event) => {
                          if (event.target !== event.currentTarget) return;
                          if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              onClick();
                          }
                      }
                    : undefined
            }>
            {children}
        </div>
    );
}

/** Trailing secondary text on a row — counts, timestamps, hints. */
export function PanelRowHint({ children }: { children: ReactNode }) {
    return <span className="panel-row-hint">{children}</span>;
}

export interface EmptyStateAction {
    label: string;
    onClick: () => void;
}

/**
 * What a panel shows instead of rows.
 *
 * `variant="inline"` is the one-line form the rails use, where a full
 * icon-and-title block would dwarf the group it sits in.
 */
export function EmptyState({
    icon,
    title,
    message,
    action,
    tone = "neutral",
    variant = "block",
}: {
    icon?: ReactNode;
    title?: string;
    /** The only required copy — `title` and `icon` are for roomier panels. */
    message: string;
    action?: EmptyStateAction;
    tone?: "neutral" | "error";
    variant?: "block" | "inline";
}) {
    if (variant === "inline") {
        const body = (
            <>
                {icon}
                <span>{message}</span>
            </>
        );
        return action ? (
            <button type="button" className={`empty-state inline tone-${tone} interactive`} onClick={action.onClick}>
                {body}
            </button>
        ) : (
            <div className={`empty-state inline tone-${tone}`}>{body}</div>
        );
    }
    return (
        <div className={`empty-state tone-${tone}`} role={tone === "error" ? "alert" : undefined}>
            {icon && (
                <span className="empty-state-media" aria-hidden="true">
                    {icon}
                </span>
            )}
            {title && <span className="empty-state-title">{title}</span>}
            <span className="empty-state-msg">{message}</span>
            {action && (
                <button type="button" className="empty-state-action" onClick={action.onClick}>
                    {action.label}
                </button>
            )}
        </div>
    );
}

/** A small status pill. Used for counts, ranges, filters and states. */
export function Badge({ tone = "neutral", children }: { tone?: "neutral" | "accent" | "warn" | "danger" | "live"; children: ReactNode }) {
    return <span className={`badge badge-${tone}`}>{children}</span>;
}
