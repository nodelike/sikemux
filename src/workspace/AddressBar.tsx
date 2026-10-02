import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { browserApi, takeKeyboardFromPages, type AddressSuggestions } from "../api/browser";
import { swallow } from "../state/toast";
import { IconLock, IconSearch } from "../ui/Icons";
import { SiteIcon } from "../ui/SiteIcon";

interface Row {
    url: string;
    title: string;
    detail: string;
    icon: string | null;
    search: boolean;
}

interface Place {
    left: number;
    top: number;
    width: number;
}

/** What the address bar shows while nobody is editing it: the address without its scheme, the site picked out. */
function siteOf(url: string): { host: string; path: string; secure: boolean } | null {
    try {
        const parsed = new URL(url);
        if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
        const path = parsed.pathname + parsed.search + parsed.hash;
        return { host: parsed.host.replace(/^www\./, ""), path: path === "/" ? "" : path, secure: parsed.protocol === "https:" };
    } catch {
        return null;
    }
}

/** The most visited sites, bar the page already open. */
function topSites(found: AddressSuggestions | null, pageAddress: string): Row[] {
    return (found?.pages ?? [])
        .filter((page) => page.url !== pageAddress)
        .map((page) => ({ url: page.url, title: page.title, detail: page.address, icon: page.icon, search: false }));
}

function rowsFor(typed: string, found: AddressSuggestions | null, suffix: string): Row[] {
    if (!found || !typed.trim()) return [];
    const searchRow: Row = { url: found.searchUrl, title: typed, detail: "Google Search", icon: null, search: true };
    const rows: Row[] = [];
    if (found.completion && suffix) {
        const { url, title, icon } = found.completion;
        rows.push({ url, title, detail: typed + suffix, icon, search: false });
    } else if (found.searches) {
        rows.push(searchRow);
    } else {
        rows.push({ url: typed, title: typed, detail: "", icon: null, search: false });
    }
    for (const page of found.pages)
        rows.push({ url: page.url, title: page.title || page.address, detail: page.address, icon: page.icon, search: false });
    if (!rows[0].search && found.searches) rows.push(searchRow);
    return rows;
}

/** Bolds each typed word where it appears, the way the bar found the row. */
function Marked({ text, words }: { text: string; words: string[] }) {
    if (!words.length) return <>{text}</>;
    const pattern = new RegExp(`(${words.map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
    return <>{text.split(pattern).map((part, index) => (index % 2 ? <b key={index}>{part}</b> : part))}</>;
}

/**
 * The address field, which finishes a remembered site in place as it is typed
 * and lists the pages that match below it. A floating one sits in a panel of
 * its own, takes the keyboard as it opens, and says when it is done. While
 * one is open the bar in the toolbar stands vacant, so the address shows once.
 */
export function AddressBar({
    tabId,
    pageAddress,
    onGo,
    floating = false,
    vacant = false,
    onLeave,
}: {
    tabId: string | undefined;
    pageAddress: string;
    onGo: (url: string) => void;
    floating?: boolean;
    vacant?: boolean;
    onLeave?: () => void;
}) {
    const listId = useId();
    const fieldRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);
    const menuRef = useRef<HTMLUListElement>(null);
    const asked = useRef(0);
    /* The bar follows the page until someone starts typing in it, and goes back
       to following once they are done. Pages move on their own — a click inside
       a web app changes the address — and that must not eat a half-typed one. */
    const [typed, setTyped] = useState<string | null>(null);
    const [found, setFound] = useState<AddressSuggestions | null>(null);
    /* Only typing forward is finished for you; deleting the finished part must
       not bring it straight back. */
    const [completing, setCompleting] = useState(false);
    const [selected, setSelected] = useState(0);
    /* Focusing the field without typing, or emptying it, offers the sites
       visited most, with none of them picked until the arrow keys pick one. */
    const [browsing, setBrowsing] = useState(false);
    const [place, setPlace] = useState<Place | null>(null);

    const completion = found?.completion?.address ?? "";
    const suffix =
        completing && typed && completion.length > typed.length && completion.toLowerCase().startsWith(typed.toLowerCase())
            ? completion.slice(typed.length)
            : "";
    const idle = browsing && !typed?.trim();
    const rows = idle ? topSites(found, pageAddress) : typed === null ? [] : rowsFor(typed, found, suffix);
    const open = idle ? rows.length > 0 : rows.length > 1 || !!suffix;
    const choice = open ? rows[selected] : undefined;
    const first = idle ? -1 : 0;
    const words = (typed ?? "").toLowerCase().split(/\s+/).filter(Boolean);
    const site = typed === null && !vacant ? siteOf(pageAddress) : null;
    const value =
        choice && selected > first
            ? choice.search
                ? choice.title
                : choice.detail || choice.url
            : typed === null
              ? vacant
                  ? ""
                  : pageAddress
              : typed + suffix;

    const reset = useCallback(() => {
        asked.current += 1;
        setTyped(null);
        setFound(null);
        setSelected(0);
        setBrowsing(false);
    }, []);

    useEffect(reset, [tabId, reset]);

    const ask = (text: string) => {
        const asking = ++asked.current;
        void browserApi
            .suggest(text)
            .then((next) => {
                if (asking === asked.current) setFound(next);
            })
            .catch(() => {});
    };

    const edit = (text: string, forward: boolean) => {
        setTyped(text);
        setCompleting(forward);
        setSelected(text.trim() ? 0 : -1);
        setBrowsing(true);
        ask(text);
    };

    const go = (url: string) => {
        reset();
        onGo(url);
        onLeave?.();
    };

    useEffect(() => {
        if (!floating) return;
        void takeKeyboardFromPages()
            .catch(swallow("take keyboard from pages"))
            .then(() => inputRef.current?.focus());
    }, [floating]);

    useLayoutEffect(() => {
        if (suffix && selected === 0) inputRef.current?.setSelectionRange(value.length - suffix.length, value.length);
    }, [suffix, selected, value]);

    useLayoutEffect(() => {
        const field = fieldRef.current;
        if (!open || floating || !field) return setPlace(null);
        const rect = field.getBoundingClientRect();
        const next = { left: rect.left, top: rect.bottom, width: rect.width };
        setPlace((previous) =>
            previous && previous.left === next.left && previous.top === next.top && previous.width === next.width ? previous : next,
        );
    }, [open, floating, rows.length]);

    /* The field's own blur misses some ways of going elsewhere: a click on
       something that takes no focus, and a click on the page, which is a view of
       its own and only shows up as the window losing focus. */
    useEffect(() => {
        if (!open && !floating) return;
        const inside = (target: EventTarget | null) =>
            target instanceof Node && (!!fieldRef.current?.contains(target) || !!menuRef.current?.contains(target));
        const leave = () => {
            inputRef.current?.blur();
            reset();
            onLeave?.();
        };
        const leaveUnlessInside = (event: Event) => {
            if (!inside(event.target)) leave();
        };
        document.addEventListener("pointerdown", leaveUnlessInside, true);
        document.addEventListener("focusin", leaveUnlessInside);
        window.addEventListener("blur", leave);
        return () => {
            document.removeEventListener("pointerdown", leaveUnlessInside, true);
            document.removeEventListener("focusin", leaveUnlessInside);
            window.removeEventListener("blur", leave);
        };
    }, [open, floating, reset, onLeave]);

    const list = open && (
        <ul
            ref={menuRef}
            id={listId}
            className="address-suggestions"
            role="listbox"
            aria-label="Suggestions"
            style={place ? { left: place.left, top: place.top, width: place.width } : undefined}>
            {rows.map((row, index) => (
                <li
                    key={`${row.search}-${row.url}`}
                    id={`${listId}-${index}`}
                    role="option"
                    aria-selected={index === selected}
                    className={index === selected ? "selected" : undefined}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => go(row.url)}>
                    <span className="address-suggestion-icon">{row.search ? <IconSearch size={13} /> : <SiteIcon src={row.icon} />}</span>
                    <span className="address-suggestion-text">
                        <span className="address-suggestion-title">
                            <Marked text={row.title} words={words} />
                        </span>
                        {row.detail && (
                            <span className="address-suggestion-detail">
                                {" — "}
                                {row.search ? row.detail : <Marked text={row.detail} words={words} />}
                            </span>
                        )}
                    </span>
                </li>
            ))}
        </ul>
    );

    return (
        <>
            <div ref={fieldRef} className={`browser-address-field${open && !floating ? " open" : ""}`}>
                {floating && <IconSearch size={14} className="browser-address-icon" />}
                <input
                    ref={inputRef}
                    className="browser-address"
                    aria-label="Address and search"
                    aria-autocomplete="both"
                    aria-controls={open ? listId : undefined}
                    aria-activedescendant={choice ? `${listId}-${selected}` : undefined}
                    value={value}
                    placeholder={vacant ? undefined : "Search or enter address"}
                    spellCheck={false}
                    autoComplete="off"
                    onFocus={(event) => {
                        event.currentTarget.select();
                        setSelected(-1);
                        setBrowsing(true);
                        ask("");
                    }}
                    onBlur={() => {
                        reset();
                        onLeave?.();
                    }}
                    onChange={(event) => {
                        const input = event.currentTarget;
                        const kind = (event.nativeEvent as InputEvent).inputType ?? "";
                        edit(input.value, !kind.startsWith("delete") && input.selectionStart === input.value.length);
                    }}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") {
                            event.preventDefault();
                            event.currentTarget.blur();
                        } else if (event.key === "Enter") {
                            event.preventDefault();
                            go(choice?.url ?? value);
                        } else if (open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
                            event.preventDefault();
                            setSelected((index) => Math.min(rows.length - 1, Math.max(first, index + (event.key === "ArrowDown" ? 1 : -1))));
                        }
                    }}
                />
                {site && (
                    <span className="browser-address-site" aria-hidden="true">
                        {site.secure && <IconLock size={11} />}
                        <span className="browser-address-host">{site.host}</span>
                        <span className="browser-address-path">{site.path}</span>
                    </span>
                )}
                {!floating && list && place && createPortal(list, document.body)}
            </div>
            {floating && list}
        </>
    );
}
