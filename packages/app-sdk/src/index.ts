/**
 * @neboai/app-sdk — zero-dep SDK for building Nebo apps.
 *
 * Mirrors native browser APIs:
 * - nebo.fetch() → fetch() with auto-routing
 * - new nebo.WebSocket() → WebSocket with auto-connect
 * - nebo.storage → localStorage-like async KV
 * - nebo.agents → invoke/stream agent responses
 * - nebo.janus → LLM completions
 * - nebo.surfaces → real-time agent events (AG-UI)
 * - nebo.a2ui → agent-driven UI via @a2ui/web_core (A2UI v0.9)
 */

import { NeboSDK } from './sdk';

export const nebo = new NeboSDK();

export { NeboSDK } from './sdk';
export { NeboWebSocket } from './websocket';
export { NeboSurfaces, surfaces } from './surfaces';
export { NeboA2UI, a2ui } from './a2ui';
export { storage } from './storage';
export { agents } from './agents';
export { janus } from './janus';
export { neboFetch } from './fetch';
export { setAppId, setBaseUrl, getAppId, getBaseUrl } from './config';
export type { AgentResponse, InvokeOptions, JanusMessage, JanusOptions, StreamChunk } from './types';
export type { A2UIMessageProcessor } from './a2ui';
export type {
  SurfaceEvent, NeboSurfaceEvent, SurfaceEventMap,
  RunStartedEvent, RunFinishedEvent, RunErrorEvent,
  TextStartEvent, TextContentEvent, TextEndEvent,
  ToolCallStartEvent, ToolCallEndEvent,
  StateSnapshotEvent, StateDeltaEvent,
  SurfaceCreateEvent, SurfaceUpdateEvent, SurfaceDeleteEvent,
  DataUpdateEvent, CustomEvent,
} from './surfaces';
