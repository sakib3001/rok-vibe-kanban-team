export type BrowserNotificationPermission =
  | NotificationPermission
  | 'unsupported';

export interface BrowserNotificationPayload {
  id: string;
  title: string;
  body: string;
  deeplinkPath?: string;
}

export function getBrowserNotificationPermission(): BrowserNotificationPermission {
  if (typeof window === 'undefined' || !('Notification' in window)) {
    return 'unsupported';
  }

  return window.Notification.permission;
}

export async function requestBrowserNotificationPermission(): Promise<BrowserNotificationPermission> {
  if (typeof window === 'undefined' || !('Notification' in window)) {
    return 'unsupported';
  }

  return window.Notification.requestPermission();
}

export function showBrowserNotification({
  id,
  title,
  body,
  deeplinkPath,
}: BrowserNotificationPayload): void {
  if (getBrowserNotificationPermission() !== 'granted') {
    return;
  }

  const notification = new window.Notification(title, {
    body,
    icon: '/favicon.png',
    tag: id,
  });

  notification.onclick = () => {
    window.focus();
    notification.close();

    if (deeplinkPath) {
      window.location.assign(deeplinkPath);
    }
  };
}
