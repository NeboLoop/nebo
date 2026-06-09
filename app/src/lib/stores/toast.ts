import { writable } from 'svelte/store';

export type ToastType = 'success' | 'error' | 'info' | 'warning';

/** Optional clickable action rendered in the toast (opens `url` in a new tab). */
export interface ToastAction {
  label: string;
  url: string;
}

export interface Toast {
  id: number;
  type: ToastType;
  message: string;
  action?: ToastAction;
}

let nextId = 0;

export const toasts = writable<Toast[]>([]);

export function addToast(
  message: string,
  type: ToastType = 'info',
  duration = 3000,
  action?: ToastAction,
) {
  const id = nextId++;
  toasts.update(t => [...t, { id, type, message, action }]);
  if (duration > 0) {
    setTimeout(() => removeToast(id), duration);
  }
}

export function removeToast(id: number) {
  toasts.update(t => t.filter(toast => toast.id !== id));
}
