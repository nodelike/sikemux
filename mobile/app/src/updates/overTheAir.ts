import { useEffect } from 'react';
import { AppState } from 'react-native';
import * as Updates from 'expo-updates';

import { CHECK_EVERY_MS, UpdateController } from './controller';

const controller = new UpdateController({
  check: () => Updates.checkForUpdateAsync(),
  fetch: () => Updates.fetchUpdateAsync(),
  reload: () => Updates.reloadAsync(),
  now: () => Date.now(),
});

/** Keeps the app's code current while it runs, and says when getting an update failed. */
export function useOverTheAirUpdates() {
  const { isUpdatePending, checkError, downloadError } = Updates.useUpdates();

  useEffect(() => {
    if (isUpdatePending) controller.downloaded();
  }, [isUpdatePending]);

  useEffect(() => {
    if (checkError) console.warn('sikemux: could not check for an update', checkError);
  }, [checkError]);

  useEffect(() => {
    if (downloadError) console.warn('sikemux: could not download an update', downloadError);
  }, [downloadError]);

  useEffect(() => {
    if (!Updates.isEnabled) return;
    if (Updates.isEmergencyLaunch)
      console.warn('sikemux: an update failed to start, so the build’s own code runs', Updates.emergencyLaunchReason);
    let polling: ReturnType<typeof setInterval> | undefined;
    const poll = () => {
      clearInterval(polling);
      polling = setInterval(() => void controller.check(), CHECK_EVERY_MS);
    };
    if (AppState.currentState === 'active') poll();
    const following = AppState.addEventListener('change', (state) => {
      if (state === 'active') {
        controller.foreground();
        poll();
      } else if (state === 'background') {
        controller.background();
        clearInterval(polling);
      }
    });
    return () => {
      following.remove();
      clearInterval(polling);
    };
  }, []);
}

/** Whether a fetched update waits for the app to restart. */
export function useUpdateWaiting(): boolean {
  return Updates.useUpdates().isUpdatePending;
}

export function restartToUpdate() {
  void controller.restart();
}
