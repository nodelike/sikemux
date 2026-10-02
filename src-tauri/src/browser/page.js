// Runs inside a browser tab on behalf of the agent. Elements the agent may
// act on are numbered by `state` and looked up by that number afterwards.
// It runs in a script world of its own: it shares the page's DOM, but the
// page's scripts can neither see it nor replace the built-ins it uses.
(() => {
    if (window.__sikemux) return;
    const INTERACTIVE =
        'a[href], button, input, select, textarea, summary, label, [role="button"], [role="link"], [role="tab"], [role="menuitem"], [role="checkbox"], [role="radio"], [role="option"], [role="switch"], [role="textbox"], [role="combobox"], [contenteditable="true"], [onclick], [tabindex]:not([tabindex="-1"])';
    // Controls in their own right. Anything else in INTERACTIVE, such as a
    // label or a div with a click handler, may just be part of a control.
    const CONTROL = 'a[href], button, input, select, textarea, summary, iframe, frame, [role], [contenteditable="true"]';
    const TEXT_CAP = 2000;
    const FULL_TEXT_CAP = 40000;
    const MAX_FOUND = 30;
    const MAX_OFFSCREEN = 40;
    const MAX_MARK_SHARE = 0.4;
    const IMPLICIT_ROLES = { A: "link", BUTTON: "button", SUMMARY: "button", SELECT: "combobox", TEXTAREA: "textbox", OPTION: "option" };
    const INPUT_ROLES = { checkbox: "checkbox", radio: "radio", button: "button", submit: "button", reset: "button", image: "button", range: "slider", search: "searchbox" };

    const compact = (value) => String(value ?? "").replace(/\s+/g, " ").trim();
    const shown = (element) => {
        const rect = element.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) return false;
        const style = getComputedStyle(element);
        return style.visibility !== "hidden" && style.display !== "none" && style.opacity !== "0";
    };
    const inViewport = (rect) => rect.bottom > 0 && rect.right > 0 && rect.top < innerHeight && rect.left < innerWidth;
    const offscreenBy = (rect) => {
        if (inViewport(rect)) return null;
        if (rect.bottom <= 0) return { side: "above", distance: -rect.bottom };
        if (rect.top >= innerHeight) return { side: "below", distance: rect.top - innerHeight };
        return { side: "to the side", distance: rect.right <= 0 ? -rect.right : rect.left - innerWidth };
    };
    const isFrame = (element) => element.tagName === "IFRAME" || element.tagName === "FRAME";
    // A frame from the same site can be read; another site's frame cannot.
    const frameDocument = (frame) => {
        try {
            return frame.contentDocument;
        } catch {
            return null;
        }
    };
    // Where an element's own viewport sits in the tab, summed over every frame
    // it is nested in, so a point inside a frame can be clicked from the top.
    const frameOffset = (element) => {
        let x = 0;
        let y = 0;
        for (let view = element.ownerDocument.defaultView; view && view !== window; ) {
            const frame = view.frameElement;
            if (!frame) break;
            const rect = frame.getBoundingClientRect();
            const style = getComputedStyle(frame);
            x += rect.left + frame.clientLeft + parseFloat(style.paddingLeft);
            y += rect.top + frame.clientTop + parseFloat(style.paddingTop);
            view = frame.ownerDocument.defaultView;
        }
        return { x, y };
    };
    const rectOf = (element) => {
        const rect = element.getBoundingClientRect();
        const offset = frameOffset(element);
        return { left: rect.left + offset.x, top: rect.top + offset.y, right: rect.right + offset.x, bottom: rect.bottom + offset.y, width: rect.width, height: rect.height };
    };
    const elementAt = (x, y) => {
        let found = document.elementFromPoint(x, y);
        while (found) {
            let deeper = null;
            if (found.shadowRoot) {
                const offset = frameOffset(found);
                deeper = found.shadowRoot.elementFromPoint(x - offset.x, y - offset.y);
            } else if (isFrame(found)) {
                const inner = frameDocument(found);
                if (inner && inner.documentElement) {
                    const offset = frameOffset(inner.documentElement);
                    deeper = inner.elementFromPoint(x - offset.x, y - offset.y);
                }
            }
            if (!deeper || deeper === found) break;
            found = deeper;
        }
        return found;
    };
    const parentAcross = (node) => {
        if (node.parentElement) return node.parentElement;
        const root = node.getRootNode();
        if (root.host) return root.host;
        return (root.defaultView && root.defaultView.frameElement) || null;
    };
    const within = (inner, outer) => {
        for (let node = inner; node; node = parentAcross(node)) if (node === outer) return true;
        return false;
    };
    // The part of an element a person can see: its box cut by every scrolling
    // or clipping container around it, and by the viewport.
    const visibleRect = (element) => {
        let { left, top, right, bottom } = rectOf(element);
        let fixed = getComputedStyle(element).position === "fixed";
        for (let node = parentAcross(element); node && left < right && top < bottom; node = parentAcross(node)) {
            const style = getComputedStyle(node);
            const root = node === node.ownerDocument.documentElement || node === node.ownerDocument.body;
            const clips = isFrame(node) || (!fixed && !root && (style.overflowX !== "visible" || style.overflowY !== "visible"));
            if (isFrame(node)) fixed = false;
            if (clips) {
                const clip = rectOf(node);
                left = Math.max(left, clip.left);
                top = Math.max(top, clip.top);
                right = Math.min(right, clip.right);
                bottom = Math.min(bottom, clip.bottom);
            }
            if (style.position === "fixed") fixed = true;
        }
        left = Math.max(left, 0);
        top = Math.max(top, 0);
        right = Math.min(right, innerWidth);
        bottom = Math.min(bottom, innerHeight);
        return right - left >= 1 && bottom - top >= 1 ? { left, top, right, bottom, width: right - left, height: bottom - top } : null;
    };
    const reaches = (element, x, y) => {
        const top = elementAt(x, y);
        if (!top) return false;
        const root = top === top.ownerDocument.documentElement || top === top.ownerDocument.body;
        return within(top, element) || (!root && within(element, top));
    };
    const PROBES = [
        [0.5, 0.5],
        [0.2, 0.2],
        [0.8, 0.2],
        [0.2, 0.8],
        [0.8, 0.8],
    ];
    // Where the element shows through, or null when it is hidden, cut off or
    // lying under something else, such as a modal's backdrop.
    const onScreen = (element) => {
        if (element.closest("[inert], [aria-hidden='true']")) return null;
        const rect = visibleRect(element);
        if (!rect) return null;
        return PROBES.some(([across, down]) => reaches(element, rect.left + rect.width * across, rect.top + rect.height * down)) ? rect : null;
    };
    const openModal = () => {
        let modal = null;
        try {
            modal = document.querySelector("dialog:modal");
        } catch {
            modal = null;
        }
        return modal || [...document.querySelectorAll('[aria-modal="true"]')].find(shown) || null;
    };
    const focused = () => {
        let element = document.activeElement;
        while (element && isFrame(element)) {
            const inner = frameDocument(element);
            if (!inner || !inner.activeElement) break;
            element = inner.activeElement;
        }
        return element;
    };
    const describe = (element) => label(element) || element.tagName.toLowerCase();
    // Styled switches often hide the real checkbox inside the label or wrapper
    // a person clicks, so its state is read from there.
    const toggleOf = (element) => {
        if (element.tagName === "INPUT" || element.hasAttribute("aria-checked") || element.hasAttribute("aria-pressed")) return element;
        if (element.tagName === "LABEL" && element.control) return element.control;
        const inner = element.querySelectorAll('input[type="checkbox"], input[type="radio"], [role="switch"], [role="checkbox"]');
        return inner.length === 1 ? inner[0] : element;
    };
    // What a field holds now, cut short; passwords never leave the page.
    const currentValue = (element) => {
        if (element.tagName === "SELECT") return compact(element.selectedOptions && element.selectedOptions[0] ? element.selectedOptions[0].textContent : element.value).slice(0, 80) || null;
        if (element.tagName !== "INPUT" && element.tagName !== "TEXTAREA") return null;
        if (["checkbox", "radio", "button", "submit", "reset", "image", "file", "hidden"].includes(element.type)) return null;
        if (!element.value) return null;
        if (element.type === "password") return "•".repeat(Math.min(element.value.length, 12));
        return compact(element.value).slice(0, 80);
    };
    const label = (element) =>
        compact(
            element.getAttribute("aria-label") ||
                labelledBy(element) ||
                (element.labels && element.labels[0] && element.labels[0].innerText) ||
                element.placeholder ||
                element.innerText ||
                element.value ||
                element.title ||
                element.alt ||
                (isFrame(element) && element.src ? `frame from ${new URL(element.src, location.href).host}` : "") ||
                "",
        ).slice(0, 96);
    // Every name an element goes by, so it can be found by any of them.
    const names = (element) =>
        [
            element.getAttribute("aria-label"),
            labelledBy(element),
            element.labels && element.labels[0] && element.labels[0].innerText,
            element.placeholder,
            element.innerText,
            element.value,
            element.title,
            element.alt,
            element.getAttribute("name"),
            element.id,
        ]
            .map((value) => (typeof value === "string" ? compact(value).toLowerCase() : ""))
            .filter(Boolean);
    const labelledBy = (element) => {
        const ids = compact(element.getAttribute("aria-labelledby"));
        if (!ids) return "";
        return ids
            .split(" ")
            .map((id) => element.ownerDocument.getElementById(id))
            .filter(Boolean)
            .map((part) => part.innerText || part.textContent || "")
            .join(" ");
    };
    const roleOf = (element) => {
        const explicit = compact(element.getAttribute("role")).split(" ")[0];
        if (explicit) return explicit.toLowerCase();
        if (element.tagName === "INPUT") return INPUT_ROLES[element.type] || "textbox";
        return IMPLICIT_ROLES[element.tagName] || (element.isContentEditable ? "textbox" : element.tagName.toLowerCase());
    };
    // Open shadow roots and same-site frames hold the controls on many sites;
    // querySelectorAll on the document alone would miss every one of them.
    // Another site's frame is listed whole, to be clicked into.
    const interactive = (root, out) => {
        for (const element of root.querySelectorAll("*")) {
            if (element.matches(INTERACTIVE)) out.push(element);
            if (element.shadowRoot) interactive(element.shadowRoot, out);
            if (isFrame(element)) {
                const inner = frameDocument(element);
                if (inner) interactive(inner, out);
                else out.push(element);
            }
        }
        return out;
    };
    const refs = () => window.__sikemuxRefs || new Map();
    const pick = (index) => {
        const element = refs().get(index);
        if (element && element.isConnected) return element;
        const numbers = window.__sikemuxNumbers;
        if (!numbers || index >= numbers.next) {
            const latest = numbers && numbers.next ? `; this page has handed out [0] to [${numbers.next - 1}]` : "; this page has not been read yet";
            throw new Error(`no element [${index}]${latest}. Numbers start again on every new page, so read browser_state or browser_find first`);
        }
        const was = element ? ` ("${describe(element)}")` : "";
        throw new Error(`no element [${index}]${was}: it has left the page or is hidden now, so read browser_state or browser_find again`);
    };
    // Elements named by a CSS selector, searched through open shadow roots and
    // same-site frames the way the element list is.
    const selected = (selector) => {
        const found = [];
        const search = (root) => {
            try {
                found.push(...root.querySelectorAll(selector));
            } catch {
                throw new Error(`"${selector}" is not a valid CSS selector`);
            }
            for (const element of root.querySelectorAll("*")) {
                if (element.shadowRoot) search(element.shadowRoot);
                if (isFrame(element)) {
                    const inner = frameDocument(element);
                    if (inner) search(inner);
                }
            }
        };
        search(document);
        return found;
    };
    // The one element a selector names, numbered so later calls can use it.
    const numberFor = (element) => {
        const numbers = window.__sikemuxNumbers || (window.__sikemuxNumbers = { ids: new WeakMap(), next: 0 });
        if (!numbers.ids.has(element)) numbers.ids.set(element, numbers.next++);
        const id = numbers.ids.get(element);
        const current = window.__sikemuxRefs || (window.__sikemuxRefs = new Map());
        current.set(id, element);
        return id;
    };
    const bySelector = (selector) => {
        const found = selected(selector).filter(shown);
        if (!found.length) throw new Error(`nothing shown matches "${selector}"`);
        if (found.length > 1) {
            const lines = found.slice(0, 10).map((element) => entry(numberFor(element), element).line);
            throw new Error(`${found.length} shown elements match "${selector}"; nothing was done. Pass a narrower selector or one of these numbers:\n${lines.join("\n")}`);
        }
        return numberFor(found[0]);
    };
    const entry = (id, element) => {
        const tag = element.tagName.toLowerCase();
        const parts = [`[${id}]`, `<${tag}${element.type ? ` type=${element.type}` : ""}${element.name ? ` name=${compact(element.name)}` : ""}>`];
        const text = label(element);
        if (text) parts.push(text);
        if (tag === "a") parts.push(`(${compact(element.getAttribute("href")).slice(0, 80)})`);
        const filled = currentValue(element);
        if (filled != null && filled !== text) parts.push(`value="${filled}"`);
        const toggle = toggleOf(element);
        const checked = toggle.getAttribute("aria-checked") ?? toggle.getAttribute("aria-pressed");
        if (toggle.checked || checked === "true") parts.push("[checked]");
        else if (checked === "mixed") parts.push("[mixed]");
        else if (toggle.type === "checkbox" || toggle.type === "radio" || checked === "false") parts.push("[unchecked]");
        const expanded = element.getAttribute("aria-expanded");
        if (expanded === "true") parts.push("[expanded]");
        if (expanded === "false") parts.push("[collapsed]");
        if (element.getAttribute("aria-selected") === "true") parts.push("[selected]");
        if (element.disabled || element.getAttribute("aria-disabled") === "true") parts.push("[disabled]");
        if (isFrame(element)) parts.push("(another site's frame: its inside cannot be read; click it or use x,y to reach in)");
        const key = parts.join(" ");
        const offscreen = offscreenBy(rectOf(element));
        return { key, line: offscreen ? `${key} [offscreen]` : key, offscreen };
    };
    // Lying under a modal, a banner or a menu, or shut off by the page.
    const covered = (element) => {
        if (element.closest("[inert], [aria-hidden='true']")) return true;
        if (!inViewport(rectOf(element))) return false;
        const rect = visibleRect(element);
        return Boolean(rect) && !PROBES.some(([across, down]) => reaches(element, rect.left + rect.width * across, rect.top + rect.height * down));
    };
    // An element keeps its number for as long as it stays on the page, so a
    // number read a moment ago never lands on a neighbour after a re-render.
    // Covered elements and the parts of a listed control keep a number but are
    // left out of the list.
    const listing = () => {
        const numbers = window.__sikemuxNumbers || (window.__sikemuxNumbers = { ids: new WeakMap(), next: 0 });
        const listed = new Map();
        const current = new Map();
        const present = interactive(document, []).filter(shown);
        const reachable = new Set(present.filter((element) => !covered(element)));
        const insideReachable = (element) => {
            for (let node = parentAcross(element); node; node = parentAcross(node)) if (reachable.has(node)) return true;
            return false;
        };
        const partOfAnother = (element) =>
            (!element.matches(CONTROL) && insideReachable(element)) || (element.tagName === "LABEL" && reachable.has(element.control));
        for (const element of present) {
            if (!numbers.ids.has(element)) numbers.ids.set(element, numbers.next++);
            const id = numbers.ids.get(element);
            current.set(id, element);
            if (reachable.has(element) && !partOfAnother(element)) listed.set(id, entry(id, element));
        }
        window.__sikemuxRefs = current;
        return listed;
    };
    const pageLines = () =>
        String((document.body && document.body.innerText) || "")
            .split("\n")
            .map(compact)
            .filter(Boolean);
    const clip = (text, cap) => (text.length > cap ? `${text.slice(0, cap)}…` : text);
    const without = (lines, taken) => {
        const counts = new Map();
        for (const line of taken) counts.set(line, (counts.get(line) || 0) + 1);
        return lines.filter((line) => {
            const left = counts.get(line) || 0;
            if (left) counts.set(line, left - 1);
            return !left;
        });
    };
    // Every element in view is kept, but only the offscreen ones nearest the
    // view, so a long page does not flood the report; the rest are counted.
    const trim = (entries) => {
        const nearest = entries
            .filter(([, listedEntry]) => listedEntry.offscreen)
            .sort(([, a], [, b]) => a.offscreen.distance - b.offscreen.distance);
        const dropped = new Set(nearest.slice(MAX_OFFSCREEN).map(([id]) => id));
        return { kept: entries.filter(([id]) => !dropped.has(id)), dropped: nearest.slice(MAX_OFFSCREEN) };
    };
    const offscreenNote = (dropped, what) => {
        const sides = new Map();
        for (const [, listedEntry] of dropped) sides.set(listedEntry.offscreen.side, (sides.get(listedEntry.offscreen.side) || 0) + 1);
        const split = ["above", "below", "to the side"].filter((side) => sides.has(side)).map((side) => `${sides.get(side)} ${side}`);
        return `${dropped.length} more ${what} offscreen and not listed (${split.join(", ")}); scroll toward them or use browser_find`;
    };
    // Numbers the agent has not been shown yet are only listed once they come
    // into view or change, and only numbers it was shown are reported removed.
    const changes = (last, listed, lines) => {
        const found = {};
        const moved = new Set([...listed].filter(([id, now]) => !last.listed.has(id) || last.listed.get(id).key !== now.key).map(([id]) => id));
        const unseen = [...listed].filter(([id, now]) => !last.shown.has(id) && (!now.offscreen || moved.has(id)));
        const { kept, dropped } = trim(unseen);
        const keptIds = new Set(kept.map(([id]) => id));
        const elements = [...listed].filter(([id]) => keptIds.has(id) || (last.shown.has(id) && moved.has(id)));
        const removed = [...last.shown].filter((id) => !listed.has(id));
        const added = without(lines, last.lines).join("\n");
        const gone = without(last.lines, lines).join("\n");
        if (elements.length) found.elements = elements.map(([, now]) => now.line).join("\n");
        if (dropped.length) found.offscreen = offscreenNote(dropped, "new elements are");
        if (removed.length) found.removed = removed;
        if (added) found.textAdded = clip(added, TEXT_CAP);
        if (gone) found.textRemoved = clip(gone, TEXT_CAP);
        const shown = new Set([...[...last.shown].filter((id) => listed.has(id)), ...elements.map(([id]) => id)]);
        return { found: Object.keys(found).length ? found : "none", shown };
    };
    const matching = (query, role) => {
        const wanted = compact(query).toLowerCase();
        const wantedRole = role ? compact(role).toLowerCase() : null;
        const listed = listing();
        const candidates = [...refs()].filter(([id, element]) => listed.has(id) && (!wantedRole || roleOf(element) === wantedRole));
        const exact = candidates.filter(([, element]) => names(element).includes(wanted));
        const found = exact.length ? exact : candidates.filter(([, element]) => names(element).some((name) => name.includes(wanted)));
        return found.filter(([, element]) => !found.some(([, other]) => other !== element && within(other, element)));
    };
    // Spots where the words show on the page outside any numbered element,
    // such as a clickable card that is a plain div.
    const textPoints = (query) => {
        const wanted = compact(query).toLowerCase();
        const points = [];
        if (!wanted || !document.body) return points;
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node && points.length < 5; node = walker.nextNode()) {
            const at = node.data.toLowerCase().indexOf(wanted);
            if (at < 0 || !node.parentElement || !shown(node.parentElement)) continue;
            const range = document.createRange();
            range.setStart(node, at);
            range.setEnd(node, Math.min(node.data.length, at + wanted.length));
            const rect = range.getBoundingClientRect();
            if (rect.width < 1 || rect.height < 1 || rect.right <= 0 || rect.bottom <= 0 || rect.left >= innerWidth) continue;
            const point = { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2), text: compact(node.parentElement.innerText || node.data).slice(0, 96) };
            if (!inViewport(rect)) point.offscreen = true;
            points.push(point);
        }
        return points;
    };
    const centre = (element) => {
        const rect = rectOf(element);
        return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    };
    const selectContents = (element) => {
        if (element.tagName === "INPUT" || element.tagName === "TEXTAREA") {
            element.select();
            return element.value.length > 0;
        }
        const range = element.ownerDocument.createRange();
        range.selectNodeContents(element);
        const selection = element.ownerDocument.defaultView.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        return compact(element.textContent).length > 0;
    };

    // What the agent draws over the page: a pointer that follows its actions,
    // a ripple where it clicks, boxes and captions it places. The layer ignores
    // the pointer, so clicks and hit tests pass straight through it.
    const OVERLAY_STYLE = [
        ":host { all: initial; }",
        ".layer { position: fixed; inset: 0; pointer-events: none; z-index: 2147483647; font: 600 12px/1.3 -apple-system, system-ui, sans-serif; }",
        ".layer.quiet .pointer, .layer.quiet .ripple { visibility: hidden; }",
        ".pointer { position: absolute; left: 0; top: 0; width: 18px; height: 18px; margin: -3px 0 0 -3px; transition: transform 220ms cubic-bezier(.2,.7,.3,1), opacity 400ms; opacity: 0; }",
        ".pointer.shown { opacity: 1; }",
        ".ripple { position: absolute; width: 36px; height: 36px; margin: -18px 0 0 -18px; border-radius: 50%; border: 2px solid #ff4f7b; animation: ripple 520ms ease-out forwards; }",
        "@keyframes ripple { from { transform: scale(.3); opacity: 1; } to { transform: scale(1.4); opacity: 0; } }",
        ".box { position: absolute; border: 2px solid #ff4f7b; border-radius: 3px; }",
        ".box.marks { border-width: 1px; }",
        ".tag { position: absolute; left: -2px; bottom: 100%; margin-bottom: 2px; padding: 1px 5px; border-radius: 3px; background: #ff4f7b; color: #fff; white-space: nowrap; max-width: 320px; overflow: hidden; text-overflow: ellipsis; }",
        ".box.marks .tag { bottom: auto; top: 0; margin: 0; padding: 0 3px; font-size: 10px; border-radius: 0 0 3px 0; }",
        ".caption { position: absolute; left: 50%; bottom: 28px; transform: translateX(-50%); max-width: 80%; padding: 10px 16px; border-radius: 8px; background: rgba(17, 17, 20, .86); color: #fff; font-size: 15px; font-weight: 500; text-align: center; }",
    ].join("\n");
    const POINTER_SVG =
        '<svg viewBox="0 0 18 18" width="18" height="18"><path d="M2 1.5 L2 15 L5.8 11.4 L8.4 17 L10.9 15.9 L8.4 10.5 L13.6 10.5 Z" fill="#ff4f7b" stroke="#fff" stroke-width="1.3" stroke-linejoin="round"/></svg>';
    let overlay = null;
    const layer = () => {
        if (overlay && overlay.host.isConnected) return overlay;
        const host = document.createElement("sikemux-overlay");
        const root = host.attachShadow({ mode: "closed" });
        root.innerHTML = `<style>${OVERLAY_STYLE}</style><div class="layer"><div class="pointer">${POINTER_SVG}</div></div>`;
        document.documentElement.appendChild(host);
        overlay = { host, layer: root.querySelector(".layer"), pointer: root.querySelector(".pointer"), notes: [], marks: [], fade: 0 };
        const follow = () => overlay && [...overlay.notes, ...overlay.marks].forEach(place);
        addEventListener("scroll", follow, { capture: true, passive: true });
        addEventListener("resize", follow, { passive: true });
        return overlay;
    };
    const place = (note) => {
        if (!note.element) return;
        if (!note.element.isConnected) return note.node.remove();
        const rect = visibleRect(note.element);
        if (!rect) return (note.node.style.display = "none");
        Object.assign(note.node.style, { display: "", left: `${rect.left - 3}px`, top: `${rect.top - 3}px`, width: `${rect.width + 2}px`, height: `${rect.height + 2}px` });
    };
    const box = (element, point, text, className) => {
        const node = document.createElement("div");
        node.className = className;
        if (text) {
            const tag = document.createElement("div");
            tag.className = "tag";
            tag.textContent = text;
            node.append(tag);
        }
        const note = { element, node };
        if (element) place(note);
        else Object.assign(node.style, { left: `${point.x - 14}px`, top: `${point.y - 14}px`, width: "24px", height: "24px", borderRadius: "50%" });
        layer().layer.append(node);
        return note;
    };

    const scrollerOf = (element) => {
        for (let node = element; node; node = parentAcross(node)) {
            if (node === document.body || node === document.documentElement) break;
            const style = getComputedStyle(node);
            if (/(auto|scroll|overlay)/.test(style.overflowY) && node.scrollHeight > node.clientHeight) return node;
        }
        return document.scrollingElement || document.documentElement;
    };
    const scrollState = (scroller) => ({ y: Math.round(scroller.scrollTop), height: scroller.scrollHeight, viewport: scroller.clientHeight, atBottom: scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2 });

    window.__sikemux = {
        // "changes" answers with what differs from the previous read of this
        // page, and with everything when there was none.
        state(mode, fullText) {
            const listed = listing();
            const lines = pageLines();
            const last = window.__sikemuxLast;
            const page = { url: location.href, title: document.title, scroll: { y: Math.round(scrollY), height: document.documentElement.scrollHeight, viewport: innerHeight } };
            if (mode === "changes" && last) {
                const { found, shown } = changes(last, listed, lines);
                window.__sikemuxLast = { listed, lines, shown };
                return { ...page, changes: found };
            }
            const { kept, dropped } = trim([...listed]);
            window.__sikemuxLast = { listed, lines, shown: new Set(kept.map(([id]) => id)) };
            const text = lines.join("\n");
            const cap = fullText ? FULL_TEXT_CAP : TEXT_CAP;
            return {
                ...page,
                viewport: { width: innerWidth, height: innerHeight },
                elements: kept.map(([, listedEntry]) => listedEntry.line).join("\n"),
                ...(dropped.length ? { offscreen: offscreenNote(dropped, "elements are") } : {}),
                text: clip(text, cap),
                ...(text.length > cap ? { textLength: text.length } : {}),
            };
        },
        find(query, role) {
            const found = matching(query, role);
            const result = { matches: found.length, elements: found.slice(0, MAX_FOUND).map(([id, element]) => entry(id, element).line).join("\n") };
            if (!found.length) {
                const points = textPoints(query);
                result.note = points.length ? "the words are on the page but on nothing numbered; click one of these points by x and y" : "nothing on the page shows those words";
                if (points.length) result.points = points;
                if (role) {
                    const wantedRole = compact(role).toLowerCase();
                    const listed = listing();
                    const sameRole = [...listed].filter(([id]) => roleOf(refs().get(id)) === wantedRole).slice(0, MAX_FOUND);
                    if (sameRole.length) {
                        result.note = `no ${wantedRole} is named that way; these are every ${wantedRole} on the page`;
                        result.elements = sameRole.map(([, listedEntry]) => listedEntry.line).join("\n");
                    }
                }
            }
            return result;
        },
        // Whether the page shows what an agent is waiting for. Every condition
        // given must hold at once.
        check(condition) {
            const failing = [];
            const text = () => pageLines().join("\n").toLowerCase();
            if (condition.text != null && !text().includes(compact(condition.text).toLowerCase())) failing.push(`text "${condition.text}" is not on the page`);
            if (condition.textGone != null && text().includes(compact(condition.textGone).toLowerCase())) failing.push(`text "${condition.textGone}" is still on the page`);
            if (condition.selector != null && !selected(condition.selector).some(shown)) failing.push(`nothing shown matches "${condition.selector}"`);
            if (condition.selectorGone != null && selected(condition.selectorGone).some(shown)) failing.push(`"${condition.selectorGone}" is still shown`);
            if (condition.url != null && !location.href.includes(condition.url)) failing.push(`the url is ${location.href}, which does not contain "${condition.url}"`);
            return { met: failing.length === 0, failing };
        },
        locate(text, role) {
            const found = matching(text, role);
            if (found.length === 1) return found[0][0];
            const scope = role ? ` with role ${role}` : "";
            if (!found.length) throw new Error(`nothing${scope} is labelled "${text}"; try browser_find with part of the words`);
            const listedMatches = found.slice(0, 10).map(([id, element]) => entry(id, element).line);
            throw new Error(`${found.length} elements${scope} match "${text}"; nothing was done. Pass an index, or a role or fuller text:\n${listedMatches.join("\n")}`);
        },
        // The number of the element a call names by `index`, `text` (with an
        // optional `role`) or `selector`, whichever it gives.
        resolve(target) {
            if (target.index != null) return target.index;
            if (target.selector != null) return bySelector(target.selector);
            if (target.text != null) return this.locate(target.text, target.role);
            throw new Error("pass index, text or selector");
        },
        point(target, expectLabel) {
            const index = typeof target === "number" ? target : this.resolve(target);
            const element = pick(index);
            if (expectLabel != null && !names(element).some((name) => name.includes(compact(expectLabel).toLowerCase()))) {
                throw new Error(`element [${index}] is "${describe(element)}", not "${expectLabel}"; nothing was done`);
            }
            element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
            const point = centre(element);
            const top = elementAt(point.x, point.y);
            const covered = top && !within(top, element) && !within(element, top) ? describe(top) : null;
            return { index, x: point.x, y: point.y, label: label(element), covered };
        },
        // The part of an element a person can see, scrolled into view, for a
        // picture of just that element.
        areaOf(target) {
            const index = this.resolve(target);
            const element = pick(index);
            element.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
            const rect = visibleRect(element);
            if (!rect) throw new Error(`element [${index}] ("${describe(element)}") is hidden or cut off, so there is nothing of it to capture`);
            return { index, label: describe(element), left: rect.left, top: rect.top, width: rect.width, height: rect.height };
        },
        // What a click at a point will land on, so a click by coordinates can
        // say what it hit.
        hitAt(x, y, expectLabel) {
            if (x > innerWidth || y > innerHeight) throw new Error(`${x},${y} is outside the ${innerWidth}×${innerHeight} viewport; nothing was done`);
            const top = elementAt(x, y);
            if (!top || top === document.documentElement || top === document.body) {
                if (expectLabel != null) throw new Error(`nothing but the page itself is at ${x},${y}, not "${expectLabel}"; nothing was done`);
                return { x, y, hit: null };
            }
            let control = top;
            for (let node = top; node; node = parentAcross(node)) {
                if (node.matches && node.matches(INTERACTIVE)) {
                    control = node;
                    break;
                }
            }
            const numbers = window.__sikemuxNumbers;
            const index = numbers && numbers.ids.has(control) ? numbers.ids.get(control) : null;
            const hit = { tag: control.tagName.toLowerCase(), label: describe(control), ...(index != null ? { index } : {}) };
            if (expectLabel != null) {
                const wanted = compact(expectLabel).toLowerCase();
                const named = [...names(control), ...names(top)].some((name) => name.includes(wanted));
                if (!named) throw new Error(`${x},${y} is on ${hit.tag} "${hit.label}", not "${expectLabel}"; nothing was done`);
            }
            return { x, y, hit };
        },
        focus(target, text, replace) {
            const element = target == null ? focused() : pick(typeof target === "number" ? target : this.resolve(target));
            if (!element || (element === element.ownerDocument.body && !element.isContentEditable)) throw new Error("nothing is focused, so there is nowhere to type; pass the field's index or selector, or click into it first");
            if (element.tagName === "SELECT") {
                const option = [...element.options].find((option) => option.value === text || compact(option.textContent) === compact(text));
                if (!option) throw new Error(`no option matching "${text}"; the options are: ${[...element.options].map((option) => compact(option.textContent)).slice(0, 20).join(", ")}`);
                element.value = option.value;
                element.dispatchEvent(new Event("input", { bubbles: true }));
                element.dispatchEvent(new Event("change", { bubbles: true }));
                return { selected: option.value };
            }
            const into = describe(element);
            if (target == null) return { replacing: replace ? selectContents(element) : false, into };
            element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
            if (isFrame(element)) return { replacing: false, clickFirst: centre(element), into };
            element.focus({ preventScroll: true });
            return { replacing: selectContents(element), into };
        },
        // WebKit only acts on a bare pointer move while its page is active, so
        // when the real move left nothing hovered the page is told by hand.
        hover(x, y) {
            const target = elementAt(x, y);
            if (!target) return { hovered: null };
            if (target.matches(":hover")) return { hovered: describe(target) };
            const offset = frameOffset(target);
            const init = { bubbles: true, cancelable: true, composed: true, clientX: x - offset.x, clientY: y - offset.y, view: target.ownerDocument.defaultView };
            const entered = { ...init, bubbles: false, cancelable: false };
            const path = [];
            for (let node = target; node; node = node.parentElement) path.unshift(node);
            target.dispatchEvent(new PointerEvent("pointerover", init));
            target.dispatchEvent(new MouseEvent("mouseover", init));
            for (const node of path) {
                node.dispatchEvent(new PointerEvent("pointerenter", entered));
                node.dispatchEvent(new MouseEvent("mouseenter", entered));
            }
            target.dispatchEvent(new PointerEvent("pointermove", init));
            target.dispatchEvent(new MouseEvent("mousemove", init));
            return { hovered: describe(target), note: "Sikemux is in the background, so the page got hover events but CSS :hover styles do not apply" };
        },
        // A long value shows its end, where typing lands, with its length.
        valueOf(target) {
            const element = target == null ? focused() : pick(typeof target === "number" ? target : this.resolve(target));
            if (!element || isFrame(element)) return { value: null };
            const raw = "value" in element && typeof element.value === "string" ? element.value : element.innerText;
            const value = compact(raw);
            if (element.type === "password") return { value: "•".repeat(Math.min(value.length, 12)), valueLength: value.length };
            return value.length > 400 ? { value: `…${value.slice(-400)}`, valueLength: value.length } : { value };
        },
        // A draggable element hands its drag to the system, which a synthesized
        // mouse cannot steer, so these drags are played out as DOM events.
        html5Drag(fromX, fromY, toX, toY) {
            const grabbed = elementAt(fromX, fromY);
            const source = grabbed && grabbed.closest('[draggable="true"], a[href]:not([draggable="false"]), img:not([draggable="false"])');
            if (!source) return { html5: false };
            const target = elementAt(toX, toY);
            if (!target) throw new Error("nothing is under the drop point");
            const data = new DataTransfer();
            const fire = (element, type, x, y) => {
                const init = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, dataTransfer: data };
                const event = typeof DragEvent === "function" ? new DragEvent(type, init) : new MouseEvent(type, init);
                if (!event.dataTransfer) Object.defineProperty(event, "dataTransfer", { value: data });
                return element.dispatchEvent(event);
            };
            if (!fire(source, "dragstart", fromX, fromY)) return { html5: true, dropped: false, note: "the page cancelled the drag" };
            fire(target, "dragenter", toX, toY);
            const refused = fire(target, "dragover", toX, toY);
            if (!refused) fire(target, "drop", toX, toY);
            fire(source, "dragend", toX, toY);
            return { html5: true, dropped: !refused, onto: describe(target) };
        },
        mark(x, y, ripple) {
            const { layer: root, pointer } = layer();
            // A tab nobody can see runs no transitions, which would leave the
            // pointer stuck where it started.
            pointer.style.transition = document.visibilityState === "visible" ? "" : "none";
            pointer.style.transform = `translate(${x}px, ${y}px)`;
            pointer.classList.add("shown");
            if (ripple) {
                const ripple = document.createElement("div");
                ripple.className = "ripple";
                Object.assign(ripple.style, { left: `${x}px`, top: `${y}px` });
                root.append(ripple);
                setTimeout(() => ripple.remove(), 600);
            }
            clearTimeout(overlay.fade);
            overlay.fade = setTimeout(() => pointer.classList.remove("shown"), 2500);
            return {};
        },
        annotate(index, x, y, text, durationMs) {
            const element = index == null ? null : pick(index);
            if (element || x != null) {
                if (element) element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
                const note = box(element, { x, y }, text, "box");
                overlay.notes.push(note);
                if (durationMs) setTimeout(() => note.node.remove(), durationMs);
                if (element && !onScreen(element)) return { annotated: describe(element), note: "the element is hidden or covered, so its box may not show where you expect" };
                return { annotated: element ? describe(element) : { x, y } };
            }
            if (!text) throw new Error("pass text for a caption, or an element or point to box");
            const { layer: root } = layer();
            root.querySelector(".caption")?.remove();
            const caption = document.createElement("div");
            caption.className = "caption";
            caption.textContent = text;
            root.append(caption);
            if (durationMs) setTimeout(() => caption.remove(), durationMs);
            return { caption: text };
        },
        clearAnnotations() {
            if (!overlay) return { cleared: 0 };
            const cleared = overlay.notes.length + (overlay.layer.querySelector(".caption") ? 1 : 0);
            overlay.notes.forEach((note) => note.node.remove());
            overlay.notes = [];
            overlay.layer.querySelector(".caption")?.remove();
            return { cleared };
        },
        // Numbers every element from the latest state on the page itself, so a
        // picture and the element list can be matched up.
        showMarks(visible) {
            if (overlay) overlay.marks.forEach((mark) => mark.node.remove());
            if (!visible) {
                if (overlay) overlay.marks = [];
                return {};
            }
            const modal = openModal();
            const seen = [...refs()]
                .filter(([, element]) => element.isConnected && (!modal || within(element, modal)))
                .map(([id, element]) => ({ id, element, rect: onScreen(element) }))
                .filter((mark) => mark.rect);
            const most = innerWidth * innerHeight * MAX_MARK_SHARE;
            const kept = seen.length > 1 ? seen.filter((mark) => mark.rect.width * mark.rect.height <= most) : seen;
            layer().marks = kept.map((mark) => box(mark.element, null, String(mark.id), "box marks"));
            return { marked: kept.map((mark) => mark.id) };
        },
        pageHeight() {
            return Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0);
        },
        pointerVisible(visible) {
            if (overlay) overlay.layer.classList.toggle("quiet", !visible);
            return {};
        },
        // Moves the page, or the scrolling element a target names, by deltaY
        // or to its top or bottom; a target with neither is brought into view.
        scroll(deltaY, target, to) {
            const element = target == null ? null : pick(this.resolve(target));
            const page = document.scrollingElement || document.documentElement;
            const scroller = element && (deltaY != null || to != null) ? scrollerOf(element) : page;
            if (element && deltaY == null && to == null) {
                element.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
                return { revealed: describe(element), onScreen: Boolean(onScreen(element)), ...scrollState(page) };
            }
            if (to === "top") scroller.scrollTo({ top: 0, behavior: "instant" });
            else if (to === "bottom") scroller.scrollTo({ top: scroller.scrollHeight, behavior: "instant" });
            else scroller.scrollBy({ top: deltaY ?? 600, behavior: "instant" });
            return scrollState(scroller);
        },
        extract(selector) {
            const roots = selector ? selected(selector) : [document.body];
            if (selector && roots.length === 0) throw new Error(`nothing matches "${selector}"`);
            const CAP = 40000;
            const textOf = (root) => compact((root && (root.innerText || root.textContent)) || "") || compact(root && (root.getAttribute("aria-label") || root.getAttribute("alt") || root.getAttribute("title") || root.value || ""));
            if (!selector || roots.length === 1) {
                const text = textOf(roots[0]);
                return { url: location.href, title: document.title, text: clip(text, CAP), matches: roots.length };
            }
            let room = CAP;
            const parts = [];
            for (const root of roots) {
                if (room <= 0) break;
                const text = clip(textOf(root), Math.min(room, 4000));
                room -= text.length;
                parts.push(text || `<${root.tagName.toLowerCase()}> with no text`);
            }
            const result = { url: location.href, title: document.title, matches: roots.length, parts };
            if (parts.length < roots.length) result.note = `the text ran past ${CAP} characters, so only the first ${parts.length} matches are here`;
            return result;
        },
    };
})();
