/**
 * NeboSDK class — orchestrates init + routing for all SDK modules.
 */

import { setAppId, setBaseUrl } from './config';
import { neboFetch } from './fetch';
import { NeboWebSocket } from './websocket';
import { storage } from './storage';
import { agents } from './agents';
import { janus } from './janus';
import { surfaces } from './surfaces';
import { a2ui } from './a2ui';

export class NeboSDK {
  fetch = neboFetch;
  WebSocket = NeboWebSocket;
  storage = storage;
  agents = agents;
  janus = janus;
  surfaces = surfaces;
  a2ui = a2ui;

  constructor() {
    // Wire: when surfaces receives a2ui_message, forward to a2ui processor
    surfaces._a2uiHandler = (msg) => a2ui._handleMessage(msg);

    // Wire: give a2ui the ability to send through surfaces' WebSocket
    a2ui._setSendFn((data) => surfaces._rawSend(data));
  }

  /**
   * Manually configure the SDK (optional — auto-detection works in most cases).
   */
  configure(options: { appId?: string; baseUrl?: string }): void {
    if (options.appId) setAppId(options.appId);
    if (options.baseUrl) setBaseUrl(options.baseUrl);
  }
}
