import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { IconCheck, IconChevron } from "./Icons";
import { useOccludeNativeViews } from "../state/nativeViews";
import { Tooltip } from "./Tooltip";
import "../styles/dropdown.css";
import { alsoLeaving, leavingMenu } from "../lib/motion";
import { rankBy } from "../lib/fuzzy";

export interface DropdownOption {
    value: string;
    label: string;
    detail?: string;
    className?: string;
}

export function Dropdown({
    value,
    options,
    onChange,
    label,
    icon,
    trailing,
    className,
    title,
    disabled,
    align = "left",
    menuWidth,
    search,
}: {
    value: string;
    options: readonly DropdownOption[];
    onChange: (value: string) => void;
    label?: string;
    icon?: ReactNode;
    trailing?: ReactNode;
    className?: string;
    title?: string;
    disabled?: boolean;
    align?: "left" | "right";
    menuWidth?: number;
    /** Puts a filter box at the top of the menu, with this placeholder. */
    search?: string;
}) {
    const [open, setOpen] = useState(false);
    const [index, setIndex] = useState(0);
    const [position, setPosition] = useState({ left: 0, top: 0, width: 0, maxHeight: 280 });
    const buttonRef = useRef<HTMLButtonElement>(null);
    const menuRef = useRef<HTMLDivElement>(null);
    const menuElement = useMemo(() => alsoLeaving(menuRef, leavingMenu), []);
    const prefix = useRef({ text: "", at: 0 });
    const id = useId();
    const active = options.find((option) => option.value === value);
    const [owner, setOwner] = useState<string>();
    const [query, setQuery] = useState("");
    const searchRef = useRef<HTMLInputElement>(null);
    const shown = useMemo(
        () => (search ? rankBy(query, options, (option) => [option.label, option.detail ?? ""]) : options),
        [search, query, options],
    );
    useOccludeNativeViews(open);
    const close = () => {
        setOpen(false);
        buttonRef.current?.focus();
    };
    const show = () => {
        setOwner(buttonRef.current?.closest<HTMLElement>("[data-modal-scope]")?.dataset.modalScope);
        setQuery("");
        setIndex(
            Math.max(
                0,
                options.findIndex((option) => option.value === value),
            ),
        );
        setOpen(true);
    };
    const choose = (next: number) => {
        if (!shown[next]) return;
        onChange(shown[next].value);
        close();
    };

    useLayoutEffect(() => {
        if (!open) return;
        const place = () => {
            const rect = buttonRef.current?.getBoundingClientRect();
            const menu = menuRef.current;
            if (!rect || !menu) return;
            const width = Math.min(window.innerWidth - 16, Math.max(menuWidth ?? 0, rect.width, 160));
            const desired = Math.min(menu.scrollHeight, 280);
            const below = window.innerHeight - rect.bottom - 12;
            const above = rect.top - 12;
            const up = below < desired && above > below;
            const maxHeight = Math.max(24, Math.min(280, up ? above : below));
            setPosition({
                width,
                maxHeight,
                left: Math.max(8, Math.min(align === "right" ? rect.right - width : rect.left, window.innerWidth - width - 8)),
                top: up ? Math.max(8, rect.top - Math.min(desired, maxHeight) - 5) : rect.bottom + 5,
            });
        };
        place();
        (searchRef.current ?? menuRef.current)?.focus();
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, true);
        return () => {
            window.removeEventListener("resize", place);
            window.removeEventListener("scroll", place, true);
        };
    }, [open, menuWidth, align, shown.length]);
    useEffect(() => {
        if (open) menuRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView?.({ block: "nearest" });
    }, [index, open]);
    useEffect(() => {
        if (!open) return;
        const outside = (event: PointerEvent) => {
            if (!menuRef.current?.contains(event.target as Node) && !buttonRef.current?.contains(event.target as Node)) setOpen(false);
        };
        document.addEventListener("pointerdown", outside, true);
        return () => document.removeEventListener("pointerdown", outside, true);
    }, [open]);

    return (
        <div className="dd">
            <Tooltip label={title}>
                <button
                    ref={buttonRef}
                    type="button"
                    className={`dd-btn${className ? ` ${className}` : ""}`}
                    aria-label={label ?? title}
                    aria-haspopup="listbox"
                    aria-expanded={open}
                    aria-controls={open ? id : undefined}
                    disabled={disabled || options.length === 0}
                    onClick={(event) => {
                        event.stopPropagation();
                        if (open) close();
                        else show();
                    }}
                    onKeyDown={(event) => {
                        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                            event.preventDefault();
                            show();
                        }
                    }}>
                    {icon && (
                        <span className="dd-icon" aria-hidden="true">
                            {icon}
                        </span>
                    )}
                    <span className={`dd-val${active?.className ? ` ${active.className}` : ""}`}>{active?.label ?? value}</span>
                    {trailing && <span className="dd-trailing">{trailing}</span>}
                    <IconChevron size={9} className="dd-chev" />
                </button>
            </Tooltip>
            {open &&
                createPortal(
                    <div
                        ref={menuElement}
                        id={id}
                        data-modal-owner={owner}
                        className={`dd-menu${search ? " searchable" : ""}`}
                        role="listbox"
                        tabIndex={-1}
                        aria-label={label ?? title}
                        aria-activedescendant={shown[index] ? `${id}-${index}` : undefined}
                        style={{ position: "fixed", ...position }}
                        onKeyDown={(event) => {
                            event.stopPropagation();
                            if (event.key === "Escape") {
                                event.preventDefault();
                                close();
                            } else if (event.key === "Tab") {
                                close();
                            } else if (event.key === "Enter" || (event.key === " " && !search)) {
                                event.preventDefault();
                                choose(index);
                            } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
                                event.preventDefault();
                                setIndex(
                                    event.key === "Home"
                                        ? 0
                                        : event.key === "End"
                                          ? shown.length - 1
                                          : (index + (event.key === "ArrowDown" ? 1 : -1) + shown.length) % Math.max(1, shown.length),
                                );
                            } else if (!search && event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
                                event.preventDefault();
                                const now = Date.now();
                                prefix.current = {
                                    text: (now - prefix.current.at < 700 ? prefix.current.text : "") + event.key.toLowerCase(),
                                    at: now,
                                };
                                const found = options.findIndex((option) => option.label.toLowerCase().startsWith(prefix.current.text));
                                if (found >= 0) setIndex(found);
                            }
                        }}>
                        {search && (
                            <input
                                ref={searchRef}
                                className="dd-search"
                                value={query}
                                placeholder={search}
                                aria-label={search}
                                spellCheck={false}
                                autoComplete="off"
                                onChange={(event) => {
                                    setQuery(event.target.value);
                                    setIndex(0);
                                }}
                            />
                        )}
                        {search && shown.length === 0 && <div className="dd-none">No matches</div>}
                        <div className="dd-list">
                            {shown.map((option, itemIndex) => (
                                <div
                                    key={option.value}
                                    id={`${id}-${itemIndex}`}
                                    data-index={itemIndex}
                                    role="option"
                                    aria-selected={option.value === value}
                                    className={`dd-item${itemIndex === index ? " active" : ""}`}
                                    onPointerMove={() => setIndex(itemIndex)}
                                    onClick={() => choose(itemIndex)}>
                                    <span className="dd-check">{option.value === value && <IconCheck size={11} />}</span>
                                    <span className={`dd-item-label${option.className ? ` ${option.className}` : ""}`}>
                                        {option.label}
                                        {option.detail && <small>{option.detail}</small>}
                                    </span>
                                </div>
                            ))}
                        </div>
                    </div>,
                    document.body,
                )}
        </div>
    );
}
