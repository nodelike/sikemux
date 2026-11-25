import { useAuth } from "@clerk/react";
import { platformName } from "@shared/remote/access";
import type { AccountEvent, Device } from "@sikemux/protocol";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent,
} from "react";

import { formatDay } from "@shared/lib/format";
import { api, problemText, signedInToken } from "./api.ts";
import { IconDownload, IconLaptop, IconPhone } from "@shared/ui/Icons";
import { brandMark, iconMark } from "./marks.ts";
import { ShedNotHost } from "./Shed.tsx";
import {
  Btn,
  btnClass,
  Section,
  Confirm,
  Empty,
  Page,
  Problem,
  Row,
  Rows,
  Tile,
  Txt,
  useToast,
} from "./ui.tsx";

export type Load =
  | { state: "loading" }
  | { state: "failed"; message: string }
  | { state: "loaded"; devices: Device[] };

/** The account's devices, read once and then kept current by `apply` and `reload`. */
export function useDevices(ready: boolean) {
  const { isSignedIn, getToken } = useAuth();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const reading = useRef(0);

  const read = useCallback(async () => {
    const attempt = ++reading.current;
    try {
      const token = await signedInToken(getToken, "see your devices");
      const list = await api(token).listDevices();
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
    await api(token).removeDevice(key);
    drop(key);
  };

  return { load, reload: () => void read(), apply, remove };
}

const DOWNLOAD = "https://sikemux.com";

const counted = (count: number, one: string) =>
  `${count} ${one}${count === 1 ? "" : "s"}`;

export function HostsPage({
  load,
  remove,
  staff,
  follow,
}: {
  load: Load;
  remove: (key: string) => Promise<void>;
  staff: boolean;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const say = useToast();
  if (load.state === "failed")
    return (
      <Page
        title="Hosts didn't load"
        lede="They still work with each other."
        mark={iconMark("laptop")}
        scene="teal"
        tone="failed"
        actions={<Btn onClick={() => location.reload()}>Try again</Btn>}
      >
        <div className="full-err">
          <Problem>{load.message}</Problem>
        </div>
      </Page>
    );
  if (load.state === "loading")
    return (
      <Page title="Hosts" mark={iconMark("laptop")} scene="teal">
        <Section label="Hosts" busy />
        <Section label="Phones" busy />
      </Page>
    );

  const hosts = load.devices.filter((device) => device.role === "host");
  const phones = load.devices.filter((device) => device.role === "client");
  const shed = staff ? <ShedNotHost follow={follow} /> : null;
  const phonesCard = (
    <Section label="Phones" count={phones.length || undefined}>
      {phones.length > 0 ? (
        <Rows>
          {phones.map((device) => (
            <DeviceRow key={device.key} device={device} remove={remove} />
          ))}
        </Rows>
      ) : (
        <p className="note">No phones yet. Sign in to Sikemux on yours.</p>
      )}
    </Section>
  );

  if (hosts.length === 0)
    return (
      <Page
        title="Add your first host"
        lede="Get Sikemux on your computer and sign in. It appears here by itself."
        mark={brandMark}
        scene="teal"
        actions={
          <a className={btnClass({})} href={DOWNLOAD}>
            <IconDownload size={16} />
            Get Sikemux for Mac
          </a>
        }
      >
        <Section label="Hosts">
          <Empty>Windows and Linux are ready too, at sikemux.com.</Empty>
        </Section>
        {phonesCard}
        {shed}
      </Page>
    );

  return (
    <Page
      title={
        phones.length > 0
          ? `${counted(hosts.length, "host")}, ${counted(phones.length, "phone")}`
          : counted(hosts.length, "host")
      }
      lede={
        hosts.length + phones.length > 1
          ? "They find each other because they share this account."
          : "Sign in on another and they find each other."
      }
      mark={iconMark("laptop")}
      scene="teal"
      actions={
        <Btn onClick={() => say("Sign in on it and it appears here.")}>
          Add a host
        </Btn>
      }
    >
      <Section label="Hosts" count={hosts.length}>
        <Rows>
          {hosts.map((device) => (
            <DeviceRow key={device.key} device={device} remove={remove} />
          ))}
        </Rows>
      </Section>
      {phonesCard}
      {shed}
    </Page>
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
  const removeButton = useRef<HTMLDivElement>(null);
  const say = useToast();
  const confirmId = `remove-${device.key}`;
  const host = device.role === "host";

  const cancel = () => {
    setConfirming(false);
    setProblem(undefined);
    removeButton.current?.querySelector("button")?.focus();
  };

  const confirm = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await remove(device.key);
      say(`Removed ${device.name}.`);
    } catch (error) {
      setProblem(problemText(error));
      setBusy(false);
    }
  };

  return (
    <Row
      wrap
      lead={
        <Tile>{host ? <IconLaptop size={16} /> : <IconPhone size={16} />}</Tile>
      }
      name={device.name}
      detail={
        platformName(device.platform) +
        (device.channel ? ` · ${device.channel}` : "") +
        ` · added ${formatDay(device.createdAt, "short")}`
      }
      end={
        <div className="ends" ref={removeButton}>
          <code className="mono" title={device.key}>
            {device.key.slice(0, 8)}
          </code>
          <Txt
            bad
            aria-label={`Remove ${device.name}`}
            aria-expanded={confirming}
            aria-controls={confirming ? confirmId : undefined}
            onClick={() => (confirming ? cancel() : setConfirming(true))}
            disabled={busy}
          >
            Remove
          </Txt>
        </div>
      }
    >
      {confirming ? (
        <Confirm
          id={confirmId}
          problem={problem}
          busy={busy}
          label="Remove"
          danger
          onCancel={cancel}
          onConfirm={() => void confirm()}
        >
          <p>
            Remove <b>{device.name}</b>? {CONSEQUENCE[device.role]}
          </p>
        </Confirm>
      ) : null}
    </Row>
  );
}
