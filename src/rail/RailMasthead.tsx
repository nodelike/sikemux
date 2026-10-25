import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { swallow } from "../state/toast";
import { Logo } from "@shared/ui/Icons";
import { RailToggle } from "./RailToggle";
import { Tooltip } from "../ui/Tooltip";
import { splitVersion } from "@shared/lib/format";

function useAppVersion(): string | null {
    const [version, setVersion] = useState<string | null>(null);
    useEffect(() => {
        getVersion().then(setVersion).catch(swallow("getVersion"));
    }, []);
    return version;
}

function VersionLabel({ version }: { version: string }) {
    const { release, channel } = splitVersion(version);
    return (
        <Tooltip label={`Sikemux ${version}`}>
            <span className="rail-masthead-version">
                v{release}
                {channel && <span className="rail-masthead-channel">{channel}</span>}
            </span>
        </Tooltip>
    );
}

export function RailMasthead() {
    const version = useAppVersion();
    return (
        <div className="rail-masthead">
            <Logo size={14} className="rail-masthead-logo" />
            <span className="rail-masthead-name">Sikemux</span>
            {version && <VersionLabel version={version} />}
            <span className="rail-masthead-actions">
                <RailToggle edge="start" />
            </span>
        </div>
    );
}
