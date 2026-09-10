import { useId, useLayoutEffect, type RefObject } from "react";

const scopes: HTMLElement[] = [];
const selector =
    'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])';

export function useModalFocus(ref: RefObject<HTMLElement | null>, enabled = true): void {
    const id = useId();
    useLayoutEffect(() => {
        const root = ref.current;
        if (!root || !enabled) return;
        const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        const inert = new Map<HTMLElement, boolean>();
        root.dataset.modalScope = id;
        scopes.push(root);
        for (let child: HTMLElement = root; child.parentElement; child = child.parentElement) {
            for (const sibling of child.parentElement.children) {
                if (sibling === child || !(sibling instanceof HTMLElement)) continue;
                inert.set(sibling, sibling.hasAttribute("inert"));
                sibling.setAttribute("inert", "");
            }
        }
        const owned = () => [...document.querySelectorAll<HTMLElement>("[data-modal-owner]")].filter((element) => element.dataset.modalOwner === id);
        const contains = (element: Node | null) => !!element && (root.contains(element) || owned().some((portal) => portal.contains(element)));
        const focusable = () =>
            [root, ...owned()]
                .flatMap((element) => [...element.querySelectorAll<HTMLElement>(selector)])
                .filter(
                    (element) =>
                        element.tabIndex >= 0 &&
                        !element.closest('[hidden], [inert], [aria-hidden="true"]') &&
                        getComputedStyle(element).visibility !== "hidden" &&
                        getComputedStyle(element).display !== "none",
                );
        const focusFirst = () => (focusable()[0] ?? root).focus();
        if (!contains(document.activeElement)) focusFirst();
        const onFocus = (event: FocusEvent) => {
            if (scopes.at(-1) === root && !contains(event.target as Node)) focusFirst();
        };
        const onKey = (event: KeyboardEvent) => {
            if (scopes.at(-1) !== root || event.key !== "Tab") return;
            const elements = focusable();
            const active = document.activeElement;
            if (!elements.length) {
                event.preventDefault();
                root.focus();
            } else if (event.shiftKey && (active === elements[0] || !elements.includes(active as HTMLElement))) {
                event.preventDefault();
                elements.at(-1)?.focus();
            } else if (!event.shiftKey && (active === elements.at(-1) || !elements.includes(active as HTMLElement))) {
                event.preventDefault();
                elements[0].focus();
            }
        };
        document.addEventListener("focusin", onFocus);
        document.addEventListener("keydown", onKey);
        return () => {
            scopes.splice(scopes.indexOf(root), 1);
            delete root.dataset.modalScope;
            for (const [element, wasInert] of inert) if (!wasInert) element.removeAttribute("inert");
            document.removeEventListener("focusin", onFocus);
            document.removeEventListener("keydown", onKey);
            if (previous?.isConnected && !previous.closest("[inert]")) previous.focus();
        };
    }, [ref, id, enabled]);
}
