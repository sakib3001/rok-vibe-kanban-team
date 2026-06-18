import { useCallback, useEffect, useMemo, useState } from 'react';

const STORAGE_KEY_PREFIX = 'vibe-kanban.browser-notifications.enabled';

function getStorageKey(userId: string | null | undefined): string | null {
  return userId ? `${STORAGE_KEY_PREFIX}.${userId}` : null;
}

function readPreference(storageKey: string | null): boolean {
  if (!storageKey || typeof window === 'undefined') {
    return false;
  }

  return window.localStorage.getItem(storageKey) === 'true';
}

export function useBrowserNotificationPreference(
  userId: string | null | undefined
) {
  const storageKey = useMemo(() => getStorageKey(userId), [userId]);
  const [enabled, setEnabledState] = useState(() => readPreference(storageKey));

  useEffect(() => {
    setEnabledState(readPreference(storageKey));
  }, [storageKey]);

  const setEnabled = useCallback(
    (nextEnabled: boolean) => {
      setEnabledState(nextEnabled);

      if (!storageKey || typeof window === 'undefined') {
        return;
      }

      if (nextEnabled) {
        window.localStorage.setItem(storageKey, 'true');
      } else {
        window.localStorage.removeItem(storageKey);
      }
    },
    [storageKey]
  );

  return {
    enabled,
    setEnabled,
  };
}
