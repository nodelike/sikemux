import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { IconCheck, IconChevron } from "./Icons";
import { Tooltip } from "./Tooltip";
import "../styles/dropdown.css";

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
}) {
    const [open, setOpen] = useState(false);
    const [index, setIndex] = useState(0);
    const [position, setPosition] = useState({ left: 0, top: 0, width: 0, maxHeight: 280 });
    const buttonRef = useRef<HTMLButtonElement>(null);
    const menuRef = useRef<HTMLDivElement>(null);
    const prefix = useRef({ text: "", at: 0 });
    const id = useId();
    const active = options.find((option) => option.value === value);
    const [owner, setOwner] = useState<string>();
    const close = () => {
        setOpen(false);
        buttonRef.current?.focus();
    };
    const show = () => {
        setOwner(buttonRef.current?.closest<HTMLElement>("[data-modal-scope]")?.dataset.modalScope);
        setIndex(
            Math.max(
                0,
                options.findIndex((option) => option.value === value),
            ),
        );
        setOpen(true);
    };
    const choose = (next: number) => {
        if (options[next]) onChange(options[next].value);
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
        menuRef.current?.focus();
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, true);
        return () => {
            window.removeEventListener("resize", place);
            window.removeEventListener("scroll", place, true);
        };
    }, [open, menuWidth, align]);
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
                        ref={menuRef}
                        id={id}
                        data-modal-owner={owner}
                        className="dd-menu"
                        role="listbox"
                        tabIndex={-1}
                        aria-label={label ?? title}
                        aria-activedescendant={options[index] ? `${id}-${index}` : undefined}
                        style={{ position: "fixed", ...position }}
                        onKeyDown={(event) => {
                            event.stopPropagation();
                            if (event.key === "Escape") {
                                event.preventDefault();
                                close();
                            } else if (event.key === "Tab") {
                                close();
                            } else if (event.key === "Enter" || event.key === " ") {
                                event.preventDefault();
                                choose(index);
                            } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
                                event.preventDefault();
                                setIndex(
                                    event.key === "Home"
                                        ? 0
                                        : event.key === "End"
                                          ? options.length - 1
                                          : (index + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length,
                                );
                            } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
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
                        {options.map((option, itemIndex) => (
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
                    </div>,
                    document.body,
                )}
        </div>
    );
}
