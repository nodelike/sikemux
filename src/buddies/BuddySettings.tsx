import { SettingsRow, SettingsRows, SettingsSection } from "../settings/SettingsLayout";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { Switch } from "../ui/Controls";
import { BuddyArt } from "./Buddy";
import { BUDDY_PACKS, EVERY_PACK, EVERY_SHAPE, packById, type BuddyPack } from "./catalogue";

/* Three different shapes in three different colours say more about a pack than its name. */
function PackSample({ pack }: { pack: BuddyPack }) {
    const step = Math.max(1, Math.floor(pack.shapes.length / 3));
    return (
        <span className="buddy-card-sample">
            {[0, 1, 2].map((i) => (
                <BuddyArt
                    key={i}
                    pack={pack}
                    shape={pack.shapes[(i * step) % pack.shapes.length]}
                    color={pack.colors[i * 2]}
                    state="idle"
                    size={24}
                />
            ))}
        </span>
    );
}

function MixedSample() {
    return (
        <span className="buddy-card-sample">
            {[BUDDY_PACKS[1], BUDDY_PACKS[7], BUDDY_PACKS[8]].map((pack, i) => (
                <BuddyArt key={pack.id} pack={pack} shape={pack.shapes[0]} color={pack.colors[i + 1]} state="idle" size={24} />
            ))}
        </span>
    );
}

export function BuddySettings() {
    const { pack, shape, motion, on } = useStore((s) => s.faceStyle);
    const chosen = packById(pack);

    return (
        <SettingsSection title="Buddies">
            <SettingsRows>
                <SettingsRow
                    label="Show buddies"
                    desc="Agents get a character in place of their harness logo."
                    asLabel
                    control={<Switch checked={on} onChange={cmd.setBuddies} label="Show buddies" />}
                />
                <SettingsRow label="Pack" desc="What new agents look like. Right-click an agent to give it its own." stack>
                    <div className="buddy-cards" role="radiogroup" aria-label="Buddy pack">
                        {BUDDY_PACKS.map((candidate) => (
                            <button
                                key={candidate.id}
                                type="button"
                                role="radio"
                                aria-checked={candidate.id === pack}
                                className={`buddy-card${candidate.id === pack ? " on" : ""}`}
                                onClick={() => cmd.setBuddyStyle(candidate.id, EVERY_SHAPE)}>
                                <PackSample pack={candidate} />
                                <span className="buddy-card-name">{candidate.name}</span>
                            </button>
                        ))}
                        <button
                            type="button"
                            role="radio"
                            aria-checked={pack === EVERY_PACK}
                            className={`buddy-card${pack === EVERY_PACK ? " on" : ""}`}
                            onClick={() => cmd.setBuddyStyle(EVERY_PACK, EVERY_SHAPE)}>
                            <MixedSample />
                            <span className="buddy-card-name">Every pack</span>
                        </button>
                    </div>
                </SettingsRow>
                <SettingsRow label="Shape" desc={chosen ? "Mix gives each agent its own shape from the pack." : "Every pack mixes shapes too."} stack>
                    <div className="buddy-shapes" role="radiogroup" aria-label="Buddy shape">
                        <button
                            type="button"
                            role="radio"
                            aria-checked={shape === EVERY_SHAPE || !chosen}
                            className={`buddy-shape mix${shape === EVERY_SHAPE || !chosen ? " on" : ""}`}
                            onClick={() => cmd.setBuddyStyle(pack, EVERY_SHAPE)}>
                            Mix
                        </button>
                        {chosen?.shapes.map((candidate, i) => (
                            <button
                                key={candidate.id}
                                type="button"
                                role="radio"
                                aria-checked={candidate.id === shape}
                                aria-label={candidate.name}
                                title={candidate.name}
                                className={`buddy-shape${candidate.id === shape ? " on" : ""}`}
                                onClick={() => cmd.setBuddyStyle(pack, candidate.id)}>
                                <BuddyArt pack={chosen} shape={candidate} color={chosen.colors[i % chosen.colors.length]} state="idle" size={26} />
                            </button>
                        ))}
                    </div>
                </SettingsRow>
                <SettingsRow
                    label="Animate faces"
                    desc="Buddies blink, look around and react to what their agent is doing."
                    asLabel
                    control={<Switch checked={motion} onChange={cmd.setBuddyMotion} label="Animate faces" />}
                />
            </SettingsRows>
        </SettingsSection>
    );
}
