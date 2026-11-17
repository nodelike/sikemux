import { forget } from '@/devices/hub';
import { pairedDevices } from '@/devices/paired';
import { turnLockOff } from '@/lock/lock';
import { choose } from '@/notify/setting';
import { forgetPush } from '@/notify/token';
import { rotateDeviceKey } from '@/phone/identity';
import { goHome } from '@/ui/navigate';
import { sayFarewell, type Farewell } from './farewell';
import { forgetCursor, stopLive } from './live';

let leaving: Promise<void> | undefined;

type Leaving = {
  farewell?: Farewell | null;
  /** The server took this phone off the account. Without that, the phone comes back as a new device. */
  confirmed: boolean;
};

/**
 * The phone's half of leaving the account: forgets every paired host, asking each one it can reach to
 * unpair it, then signs out of Clerk. Two callers at once share one run.
 */
export function signOutHere(signOut: () => Promise<unknown>, { farewell = null, confirmed }: Leaving): Promise<void> {
  if (farewell) sayFarewell(farewell);
  leaving ??= (async () => {
    stopLive();
    await forgetPush().catch(() => {});
    await choose(undefined).catch(() => {});
    await turnLockOff().catch(() => {});
    const devices = await pairedDevices().catch(() => []);
    await Promise.allSettled(devices.map((device) => forget(device.core)));
    await forgetCursor().catch(() => {});
    if (!confirmed)
      await rotateDeviceKey().catch((error: unknown) => console.warn('sikemux: could not make this phone a new device', error));
    await signOut().catch((error: unknown) => console.warn('sikemux: Clerk could not sign out', error));
    goHome();
  })().finally(() => {
    leaving = undefined;
  });
  return leaving;
}
