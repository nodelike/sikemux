import { useAuth } from "@clerk/react";
import { formatDay } from "@shared/lib/format";
import { platformName } from "@shared/remote/access";
import {
  IconDownload,
  IconKey,
  IconLaptop,
  IconMore,
  IconPhone,
  IconRefresh,
  IconServer,
} from "@shared/ui/Icons";
import type { AccountEvent, Device } from "@sikemux/protocol";
import {
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent,
} from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { iconMark } from "./marks.ts";
import { HOST } from "./navigation.ts";
import { ShedNotHost } from "./pages.ts";
import {
  Btn,
  btnClass,
  Confirm,
  Empty,
  Nothing,
  Page,
  Popover,
  Problem,
  Row,
  Rows,
  Section,
  Table,
  Td,
  Tr,
  Txt,
  useAction,
  useToast,
  Who,
} from "./ui.tsx";
import "./hosts.css";

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

const shortKey = (key: string) => {
  const head = key.slice(0, 8).toUpperCase();
  return head.length > 4 ? `${head.slice(0, 4)} ${head.slice(4)}` : head;
};

const CONSEQUENCE: Record<Device["role"], string> = {
  host: "It leaves your account and stops hearing about it. The phones paired to it keep working.",
  client:
    "It is signed out and removed from every host, so it can't connect to them until it signs in and is allowed again.",
};

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
  const removeAction = useAction();
  const [removing, setRemoving] = useState<Device | null>(null);

  if (load.state === "failed")
    return (
      <Page
        title="Hosts"
        lede="Computers your agents run on."
        mark={iconMark("laptop")}
        scene="you"
        tone="failed"
      >
        <Nothing
          title="Hosts didn't load"
          actions={
            <Btn quiet onClick={() => location.reload()}>
              <IconRefresh size={14} />
              Try again
            </Btn>
          }
        >
          They still work with each other.
        </Nothing>
        <Problem>{load.message}</Problem>
      </Page>
    );

  if (load.state === "loading")
    return (
      <Page
        title="Hosts"
        lede="Computers your agents run on."
        mark={iconMark("laptop")}
        scene="you"
      >
        <Section label="Hosts" busy />
        <Section label="Phones" busy />
      </Page>
    );

  const hosts = load.devices.filter((device) => device.role === "host");
  const phones = load.devices.filter((device) => device.role === "client");
  const shed = staff ? (
    <Suspense fallback={null}>
      <ShedNotHost follow={follow} />
    </Suspense>
  ) : null;

  const askRemove = (device: Device) => {
    removeAction.clear();
    setRemoving(device);
  };

  const closeConfirm = () => {
    removeAction.clear();
    setRemoving(null);
  };

  const confirmDialog = removing ? (
    <Confirm
      title={`Remove ${removing.name}?`}
      problem={removeAction.problem}
      busy={removeAction.busy}
      label={removing.role === "host" ? "Remove host" : "Remove phone"}
      danger
      onCancel={closeConfirm}
      onConfirm={() =>
        void removeAction.run(async () => {
          const target = removing;
          await remove(target.key);
          setRemoving(null);
          say(`Removed ${target.name}.`);
        }, problemText)
      }
    >
      <p>{CONSEQUENCE[removing.role]}</p>
    </Confirm>
  ) : null;

  const phonesSection = (
    <Section label="Phones" count={phones.length || undefined}>
      {phones.length > 0 ? (
        <div className="phones-rows">
          <Rows>
            {phones.map((device) => (
              <Row
                key={device.key}
                lead={<IconPhone size={22} className="host-ic" />}
                name={device.name}
                detail={`${platformName(device.platform)} · added ${formatDay(device.createdAt, "short")}`}
                end={
                  <Txt
                    aria-label={`Remove ${device.name}`}
                    onClick={() => askRemove(device)}
                  >
                    Remove
                  </Txt>
                }
              />
            ))}
          </Rows>
        </div>
      ) : (
        <Empty>No phones yet. Sign in to Sikemux on yours.</Empty>
      )}
    </Section>
  );

  if (hosts.length === 0)
    return (
      <Page
        title="Hosts"
        lede="Computers your agents run on."
        mark={iconMark("laptop")}
        scene="you"
      >
        <Nothing
          title="Add your first host"
          actions={
            <>
              <a className={btnClass({})} href={DOWNLOAD}>
                <IconDownload size={14} />
                Get Sikemux for Mac
              </a>
              <a
                className={btnClass({ quiet: true })}
                href={HOST}
                onClick={follow(HOST)}
              >
                Enter a code
              </a>
            </>
          }
        >
          Open Sikemux on a computer and sign in, and it shows here. A Linux
          server signs in with <code className="mono">sikemux login</code>.
        </Nothing>
        {phones.length > 0 ? phonesSection : null}
        {shed}
        {confirmDialog}
      </Page>
    );

  return (
    <Page
      title="Hosts"
      lede={
        phones.length > 0
          ? `${counted(hosts.length, "host")}, ${counted(phones.length, "phone")}.`
          : `${counted(hosts.length, "host")}.`
      }
      mark={iconMark("laptop")}
      scene="you"
    >
      <Table
        label="Hosts"
        cols="minmax(0, 1fr) auto 36px"
        heads={["Host", "Key", ""]}
      >
        {hosts.map((device) => (
          <Tr key={device.key}>
            <Td>
              <Who
                lead={
                  device.platform === "linux" ? (
                    <IconServer size={22} className="host-ic" />
                  ) : (
                    <IconLaptop size={22} className="host-ic" />
                  )
                }
                name={device.name}
                detail={
                  platformName(device.platform) +
                  (device.channel ? ` · ${device.channel}` : "") +
                  ` · added ${formatDay(device.createdAt, "short")}`
                }
              />
            </Td>
            <Td wide>
              <span className="host-key" title={device.key}>
                <IconKey size={13} />
                {shortKey(device.key)}
              </span>
            </Td>
            <Td end>
              <Popover
                look="icon"
                label={`More for ${device.name}`}
                trigger={<IconMore size={16} />}
                options={[
                  {
                    key: "remove",
                    name: "Remove host",
                    detail: "It leaves your account",
                    bad: true,
                    choose: () => askRemove(device),
                  },
                ]}
              />
            </Td>
          </Tr>
        ))}
      </Table>
      {phonesSection}
      {shed}
      {confirmDialog}
    </Page>
  );
}
