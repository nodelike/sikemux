import { useAuth } from "@clerk/react";
import type { AccountEvent, Device } from "@sikemux/protocol";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { api, formatDay, problemText, signedInToken } from "./api.ts";
import { LaptopIcon, PhoneIcon } from "./icons.tsx";

export type Load =
  | { state: "loading" }
  | { state: "failed"; message: string }
  | { state: "loaded"; devices: Device[] };

const PLATFORMS: Record<Device["platform"], string> = {
  macos: "macOS",
  linux: "Linux",
  ios: "iOS",
  android: "Android",
};

/** The account's devices, read once and then kept current by `apply` and `reload`. */
export function useDevices(ready: boolean) {
  const { isSignedIn, getToken } = useAuth();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const reading = useRef(0);

  const read = useCallback(async () => {
    const attempt = ++reading.current;
    try {
      const token = await signedInToken(getToken, "see your devices");
      const list = await api.devices(token);
      if (attempt === reading.current)
        setLoad({ state: "loaded", devices: list.devices });
    } catch (error) {
      if (attempt !== reading.current) return;
      setLoad((current) =>
        current.state === "loaded"
          ? current
          : { state: "failed", message: problemText(error) },
      );
    }
  }, [getToken]);

  useEffect(() => {
    if (!ready || !isSignedIn) return;
    void read();
    return () => {
      reading.current += 1;
    };
  }, [ready, isSignedIn, read]);

  const drop = useCallback((key: string) => {
    setLoad((current) =>
      current.state === "loaded"
        ? {
            state: "loaded",
            devices: current.devices.filter((device) => device.key !== key),
          }
        : current,
    );
  }, []);

  /** Events name what changed but not what it is now, so anything but a removal rereads the list. */
  const apply = (events: AccountEvent[]) => {
    for (const event of events)
      if (event.type === "device.revoked" && event.key) drop(event.key);
    if (
      events.some(
        (event) =>
          event.type === "device.added" || event.type === "device.changed",
      )
    )
      void read();
  };

  const remove = async (key: string) => {
    const token = await signedInToken(getToken, "remove a device");
    await api.removeDevice(token, key);
    drop(key);
  };

  return { load, reload: () => void read(), apply, remove };
}

export function Devices({
  load,
  remove,
}: {
  load: Load;
  remove: (key: string) => Promise<void>;
}) {
  if (load.state === "failed") return <p className="problem">{load.message}</p>;

  const devices = load.state === "loaded" ? load.devices : null;
  return (
    <div className="groups">
      <DeviceGroup
        title="Hosts"
        empty="No hosts yet. In Sikemux on your computer, open Settings, then Devices, and sign in."
        devices={devices?.filter((device) => device.role === "host") ?? null}
        remove={remove}
      />
      <DeviceGroup
        title="Clients"
        empty="No clients yet. Sign in to Sikemux on your phone."
        devices={devices?.filter((device) => device.role === "client") ?? null}
        remove={remove}
      />
    </div>
  );
}

function DeviceGroup({
  title,
  empty,
  devices,
  remove,
}: {
  title: string;
  empty: string;
  devices: Device[] | null;
  remove: (key: string) => Promise<void>;
}) {
  return (
    <section className="group">
      <h2>
        {title}
        {devices ? <span className="count">{devices.length}</span> : null}
      </h2>
      {devices === null ? (
        <ul className="rows" aria-busy="true">
          <li className="row placeholder" />
        </ul>
      ) : devices.length === 0 ? (
        <p className="empty">{empty}</p>
      ) : (
        <ul className="rows">
          {devices.map((device) => (
            <DeviceRow key={device.key} device={device} remove={remove} />
          ))}
        </ul>
      )}
    </section>
  );
}

const CONSEQUENCE: Record<Device["role"], string> = {
  host: "It leaves your account and stops hearing about it. The phones paired to it keep working.",
  client:
    "It is signed out and removed from every host, so it can't connect to them until it signs in and is allowed again.",
};

function DeviceRow({
  device,
  remove,
}: {
  device: Device;
  remove: (key: string) => Promise<void>;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const removeButton = useRef<HTMLButtonElement>(null);
  const confirmId = `remove-${device.key}`;

  const cancel = () => {
    setConfirming(false);
    setProblem(undefined);
    removeButton.current?.focus();
  };

  const confirm = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await remove(device.key);
    } catch (error) {
      setProblem(problemText(error));
      setBusy(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !busy) cancel();
  };

  return (
    <li className="row" data-confirming={confirming || undefined}>
      <span className="glyph">
        {device.role === "host" ? (
          <LaptopIcon size={18} />
        ) : (
          <PhoneIcon size={18} />
        )}
      </span>
      <span className="name">{device.name}</span>
      <span className="detail">
        {PLATFORMS[device.platform] ?? device.platform}
        {device.channel ? ` · ${device.channel}` : ""}
        {` · added ${formatDay(device.createdAt, "short")}`}
      </span>
      <span className="end">
        <code className="key" title={device.key}>
          {device.key.slice(0, 8)}
        </code>
        <button
          ref={removeButton}
          type="button"
          className="remove"
          aria-expanded={confirming}
          aria-controls={confirming ? confirmId : undefined}
          onClick={() => (confirming ? cancel() : setConfirming(true))}
          disabled={busy}
        >
          Remove
        </button>
      </span>
      {confirming ? (
        <div className="confirm" id={confirmId} onKeyDown={onKeyDown}>
          <p>
            Remove <span className="ink">{device.name}</span>?{" "}
            {CONSEQUENCE[device.role]}
          </p>
          {problem ? (
            <p className="problem" role="alert">
              {problem}
            </p>
          ) : null}
          <div className="actions">
            <button
              type="button"
              className="button small"
              onClick={cancel}
              disabled={busy}
              autoFocus
            >
              Cancel
            </button>
            <button
              type="button"
              className="button small danger"
              onClick={() => void confirm()}
              disabled={busy}
              data-busy={busy || undefined}
            >
              Remove
            </button>
          </div>
        </div>
      ) : null}
    </li>
  );
}
