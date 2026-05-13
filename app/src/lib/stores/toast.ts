import { writable } from 'svelte/store';

export type ToastType = 'success' | 'error' | 'info' | 'warning';

export interface Toast {
  id: number;
  type: ToastType;
  message: string;
}

let nextId = 0;

export const toasts = writable<Toast[]>([]);

export function addToast(message: string, type: ToastType = 'info', duration = 3000) {
  const id = nextId++;
  toasts.update(t => [...t, { id, type, message }]);
  if (duration > 0) {
    setTimeout(() => removeToast(id), duration);
  }
}

export function removeToast(id: number) {
  toasts.update(t => t.filter(toast => toast.id !== id));
}
