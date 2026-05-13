/**
 * nebo.WebSocket — mirrors native WebSocket, auto-connects to /ws/app/{id}.
 */

import { getAppId, getBaseUrl } from './config';

export class NeboWebSocket {
  private ws: WebSocket;

  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;

  constructor(_path?: string) {
    const appId = getAppId();
    const base = getBaseUrl();
    const wsBase = base.replace(/^http/, 'ws');
    const url = `${wsBase}/ws/app/${appId}`;

    this.ws = new WebSocket(url);
    this.ws.onopen = (e) => this.onopen?.(e);
    this.ws.onmessage = (e) => this.onmessage?.(e);
    this.ws.onerror = (e) => this.onerror?.(e);
    this.ws.onclose = (e) => this.onclose?.(e);
  }

  get readyState(): number {
    return this.ws.readyState;
  }

  send(data: string | ArrayBuffer | Blob): void {
    this.ws.send(data);
  }

  close(code?: number, reason?: string): void {
    this.ws.close(code, reason);
  }
}
