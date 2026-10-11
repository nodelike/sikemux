import {
    useEffect,
    useLayoutEffect,
    useRef,
    useState,
    type ComponentType,
    type MouseEvent as ReactMouseEvent,
    type PointerEvent as ReactPointerEvent,
    type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import * as cmd from "../state/commands";
import { confirmDialog, promptDialog } from "../state/dialog";
import { SPACE_ICONS, type SpaceIcon } from "../state/projectSpaces";
import type { ProjectSpace } from "../state/types";
import {
    IconBolt,
    IconBook,
    IconBriefcase,
    IconBuilding,
    IconCheck,
    IconCode,
    IconCube,
    IconFlask,
    IconFolder,
    IconGlobe,
    IconGrip,
    IconHeart,
    IconHome,
    IconLaptop,
    IconLeaf,
    IconMusic,
    IconPlus,
    IconRocket,
    IconStack,
    IconStar,
    IconTerminal,
    IconTrash,
    IconUser,
} from "../ui/Icons";
import { TreeContextMenu, type CtxItem } from "./FileTree";

const GLYPHS: Record<SpaceIcon, ComponentType<{ size?: number }>> = {
    folder: IconFolder,
    user: IconUser,
    globe: IconGlobe,
    laptop: IconLaptop,
    building: IconBuilding,
    home: IconHome,
    briefcase: IconBriefcase,
    code: IconCode,
    flask: IconFlask,
    rocket: IconRocket,
    star: IconStar,
    heart: IconHeart,
    book: IconBook,
    bolt: IconBolt,
    leaf: IconLeaf,
    cube: IconCube,
    terminal: IconTerminal,
    music: IconMusic,
};

export function SpaceGlyph({ icon, size = 12 }: { icon: SpaceIcon | ""; size?: number }) {
    if (!icon) return null;
    const Glyph = GLYPHS[icon];
    return <Glyph size={size} />;
}

async function askSpaceName(title: string, confirmLabel: string, initial = ""): Promise<string | null> {
    const name = await promptDialog({ title, label: "Name", initial, placeholder: "Work, Side projects, Client A…", confirmLabel });
    return name?.trim() ? name : null;
}

export async function newSpace(then?: (id: string) => void): Promise<void> {
    const name = await askSpaceName("Create space", "Create");
    const id = name ? cmd.createSpace(name) : null;
    if (id) (then ?? cmd.showSpace)(id);
}

const renameSpace = (space: ProjectSpace) =>
    void askSpaceName("Rename space", "Rename", space.name).then((name) => name && cmd.renameSpace(space.id, name));

const deleteSpace = (space: ProjectSpace) =>
    void confirmDialog({
        title: `Delete “${space.name}”?`,
        body: "Its projects stay open and show under All.",
        confirmLabel: "Delete",
    }).then((yes) => yes && cmd.deleteSpace(space.id));

type Point = { x: number; y: number; width?: number };
type Overlay =
    { kind: "menu"; at: Point; items: CtxItem[] } | { kind: "icons"; at: Point; spaceId: string; returnTo?: Overlay } | { kind: "editor"; at: Point };

export function SpaceSwitch({ spaces, activeSpaceId }: { spaces: readonly ProjectSpace[]; activeSpaceId: string | null }) {
    const [overlay, setOverlay] = useState<Overlay | null>(null);
    const switchRef = useRef<HTMLDivElement>(null);
    const close = () => setOverlay(null);

    const editorAt = (): Point => {
        const rect = switchRef.current?.getBoundingClientRect();
        return rect ? { x: rect.left, y: rect.bottom + 4, width: rect.width } : { x: 0, y: 0 };
    };
    const sharedItems: CtxItem[] = [
        { label: "New Space…", run: () => void newSpace() },
        { label: "Edit Spaces…", run: () => setOverlay({ kind: "editor", at: editorAt() }) },
    ];
    const menuFor = (event: ReactMouseEvent, space?: ProjectSpace) => {
        event.preventDefault();
        event.stopPropagation();
        const at = { x: event.clientX, y: event.clientY };
        const items: CtxItem[] = space
            ? [
                  { label: "Rename…", run: () => renameSpace(space) },
                  { label: "Change Icon…", run: () => setOverlay({ kind: "icons", at, spaceId: space.id }) },
                  ...(space.icon ? [{ label: "Remove Icon", run: () => cmd.setSpaceIcon(space.id, "") }] : []),
                  { sep: true },
                  ...sharedItems,
                  { sep: true },
                  { label: "Delete Space…", danger: true, run: () => deleteSpace(space) },
              ]
            : sharedItems;
        setOverlay({ kind: "menu", at, items });
    };

    const overlays = (
        <>
            {overlay?.kind === "menu" && <TreeContextMenu x={overlay.at.x} y={overlay.at.y} items={overlay.items} onClose={close} />}
            {overlay?.kind === "editor" && (
                <SpaceEditor
                    spaces={spaces}
                    at={overlay.at}
                    onClose={close}
                    onPickIcon={(spaceId, at) => setOverlay({ kind: "icons", at, spaceId, returnTo: overlay })}
                />
            )}
            {overlay?.kind === "icons" && (
                <SpaceIconPicker
                    at={overlay.at}
                    current={spaces.find((space) => space.id === overlay.spaceId)?.icon ?? ""}
                    onPick={(icon) => {
                        cmd.setSpaceIcon(overlay.spaceId, icon);
                        setOverlay(overlay.returnTo ?? null);
                    }}
                    onClose={() => setOverlay(overlay.returnTo ?? null)}
                />
            )}
        </>
    );

    if (spaces.length === 0) {
        return (
            <div className="space-switch empty">
                <button type="button" className="space-new" onClick={() => void newSpace()}>
                    <IconPlus size={11} />
                    <span>New space</span>
                </button>
            </div>
        );
    }
    return (
        <div ref={switchRef} className="space-switch">
            <SpaceTrack spaces={spaces} activeSpaceId={activeSpaceId} onMenu={menuFor} />
            {overlays}
        </div>
    );
}

function SpaceTrack({
    spaces,
    activeSpaceId,
    onMenu,
}: {
    spaces: readonly ProjectSpace[];
    activeSpaceId: string | null;
    onMenu: (event: ReactMouseEvent, space?: ProjectSpace) => void;
}) {
    const trackRef = useRef<HTMLDivElement>(null);
    const [clipped, setClipped] = useState({ start: false, end: false });

    const measure = () => {
        const track = trackRef.current;
        if (!track) return;
        const start = track.scrollLeft > 1;
        const end = track.scrollLeft + track.clientWidth < track.scrollWidth - 1;
        setClipped((was) => (was.start === start && was.end === end ? was : { start, end }));
    };

    useLayoutEffect(() => {
        const track = trackRef.current;
        const active = track?.querySelector<HTMLElement>('[aria-checked="true"]');
        if (track && active) {
            const left = active.offsetLeft - track.offsetLeft;
            const right = left + active.offsetWidth;
            if (left < track.scrollLeft) track.scrollLeft = left;
            else if (right > track.scrollLeft + track.clientWidth) track.scrollLeft = right - track.clientWidth;
        }
        measure();
    }, [activeSpaceId, spaces]);

    useEffect(() => {
        const track = trackRef.current;
        if (!track) return;
        const observer = new ResizeObserver(measure);
        observer.observe(track);
        // A mouse wheel only scrolls up and down, so it moves the track sideways instead.
        const onWheel = (event: WheelEvent) => {
            if (event.deltaX !== 0 || track.scrollWidth <= track.clientWidth) return;
            event.preventDefault();
            track.scrollLeft += event.deltaY;
        };
        track.addEventListener("wheel", onWheel, { passive: false });
        return () => {
            observer.disconnect();
            track.removeEventListener("wheel", onWheel);
        };
    }, []);

    const option = (space: ProjectSpace | null) => {
        const id = space?.id ?? null;
        return (
            <button
                key={id ?? "all"}
                type="button"
                role="radio"
                aria-checked={activeSpaceId === id}
                className={activeSpaceId === id ? "active" : ""}
                onClick={() => cmd.showSpace(id)}
                onContextMenu={(event) => onMenu(event, space ?? undefined)}
                onDoubleClick={space ? () => renameSpace(space) : undefined}>
                {space ? <SpaceGlyph icon={space.icon} /> : <IconStack size={12} />}
                <span className="space-name">{space?.name ?? "All"}</span>
            </button>
        );
    };
    return (
        <div
            ref={trackRef}
            className={`space-track${clipped.start ? " clipped-start" : ""}${clipped.end ? " clipped-end" : ""}`}
            role="radiogroup"
            aria-label="Projects shown"
            onScroll={measure}
            onContextMenu={(event) => onMenu(event)}>
            {option(null)}
            {spaces.map(option)}
        </div>
    );
}

function Popover({
    at,
    onClose,
    className,
    label,
    children,
}: {
    at: Point;
    onClose: () => void;
    className: string;
    label: string;
    children: ReactNode;
}) {
    const ref = useRef<HTMLDivElement>(null);
    const [pos, setPos] = useState({ left: at.x, top: at.y });

    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const rect = el.getBoundingClientRect();
        const pad = 6;
        setPos({
            left: Math.max(pad, Math.min(at.x, window.innerWidth - rect.width - pad)),
            top: Math.max(pad, Math.min(at.y, window.innerHeight - rect.height - pad)),
        });
    }, [at.x, at.y]);

    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            onClose();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [onClose]);

    return createPortal(
        <div
            className="tree-ctx-scrim"
            onPointerDown={(event) => event.target === event.currentTarget && onClose()}
            onContextMenu={(event) => {
                event.preventDefault();
                onClose();
            }}>
            <div
                ref={ref}
                role="dialog"
                aria-label={label}
                className={`space-pop ${className}`}
                style={{ left: pos.left, top: pos.top, width: at.width }}>
                {children}
            </div>
        </div>,
        document.body,
    );
}

function SpaceIconPicker({
    at,
    current,
    onPick,
    onClose,
}: {
    at: Point;
    current: SpaceIcon | "";
    onPick: (icon: SpaceIcon | "") => void;
    onClose: () => void;
}) {
    return (
        <Popover at={at} onClose={onClose} className="space-icon-picker" label="Space icon">
            <div className="space-icon-grid">
                {SPACE_ICONS.map((icon) => (
                    <button
                        key={icon}
                        type="button"
                        aria-label={icon}
                        aria-pressed={current === icon}
                        className={current === icon ? "active" : ""}
                        onClick={() => onPick(icon)}>
                        <SpaceGlyph icon={icon} size={14} />
                    </button>
                ))}
            </div>
            <button
                type="button"
                className={`space-icon-none${current === "" ? " active" : ""}`}
                aria-pressed={current === ""}
                onClick={() => onPick("")}>
                No icon
            </button>
        </Popover>
    );
}

function SpaceEditor({
    spaces,
    at,
    onClose,
    onPickIcon,
}: {
    spaces: readonly ProjectSpace[];
    at: Point;
    onClose: () => void;
    onPickIcon: (spaceId: string, at: Point) => void;
}) {
    const listRef = useRef<HTMLDivElement>(null);
    const [dragging, setDragging] = useState<string | null>(null);

    const startDrag = (event: ReactPointerEvent, id: string) => {
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        setDragging(id);
    };
    const drag = (event: ReactPointerEvent) => {
        if (!dragging) return;
        const rows = [...(listRef.current?.querySelectorAll<HTMLElement>(".space-edit-row") ?? [])];
        const index = rows.findIndex((row) => {
            const rect = row.getBoundingClientRect();
            return event.clientY < rect.top + rect.height / 2;
        });
        const target = index < 0 ? rows.length - 1 : index;
        if (spaces[target]?.id !== dragging) cmd.moveSpace(dragging, target);
    };

    return (
        <Popover at={at} onClose={onClose} className="space-editor" label="Edit spaces">
            <div ref={listRef} className={`space-edit-list${dragging ? " dragging" : ""}`}>
                {spaces.map((space) => (
                    <div key={space.id} className={`space-edit-row${dragging === space.id ? " lifted" : ""}`}>
                        <span
                            className="space-edit-grip"
                            aria-label={`Move ${space.name}`}
                            onPointerDown={(event) => startDrag(event, space.id)}
                            onPointerMove={drag}
                            onPointerUp={() => setDragging(null)}
                            onPointerCancel={() => setDragging(null)}>
                            <IconGrip size={12} />
                        </span>
                        <button
                            type="button"
                            className={`space-edit-icon${space.icon ? "" : " empty"}`}
                            aria-label={`Icon for ${space.name}`}
                            onClick={(event) => {
                                const rect = event.currentTarget.getBoundingClientRect();
                                onPickIcon(space.id, { x: rect.left, y: rect.bottom + 4 });
                            }}>
                            {space.icon ? <SpaceGlyph icon={space.icon} size={13} /> : <IconPlus size={11} />}
                        </button>
                        <SpaceNameField space={space} />
                        <button type="button" className="space-edit-delete" aria-label={`Delete ${space.name}`} onClick={() => deleteSpace(space)}>
                            <IconTrash size={13} />
                        </button>
                    </div>
                ))}
            </div>
            <div className="space-edit-foot">
                <button type="button" onClick={() => void newSpace(() => undefined)}>
                    <IconPlus size={11} />
                    New
                </button>
                <button type="button" className="done" onClick={onClose}>
                    <IconCheck size={11} />
                    Done
                </button>
            </div>
        </Popover>
    );
}

function SpaceNameField({ space }: { space: ProjectSpace }) {
    const [draft, setDraft] = useState(space.name);
    useEffect(() => setDraft(space.name), [space.name]);
    const commit = () => {
        if (draft.trim()) cmd.renameSpace(space.id, draft);
        else setDraft(space.name);
    };
    return (
        <input
            className="space-edit-name"
            aria-label={`Name of ${space.name}`}
            value={draft}
            spellCheck={false}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={commit}
            onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
                if (event.key === "Escape") {
                    event.stopPropagation();
                    setDraft(space.name);
                }
            }}
        />
    );
}
