# Exploratory PRD: Nebo App Windows

**Static Web Apps as First-Class Desktop Citizens**

| | |
|---|---|
| **Status** | Exploratory Draft |
| **Author** | Alma Tuck |
| **Date** | May 6, 2026 |
| **Classification** | Internal / Confidential |

---

## 1. Overview

This document explores adding a new capability to Nebo: the ability for static web applications (HTML/CSS/JS bundles) to be installed via the NeboLoop marketplace and launched as independent Tauri windows with access to a full HTTP client, WebSocket support, and the Nebo agent/tool ecosystem.

This is an exploratory PRD. Its purpose is to articulate the opportunity, define the architecture at a conceptual level, surface open questions, and establish a framework for deciding whether to move to a full specification.

---

## 2. Problem Statement

Nebo today is an AI agent platform. Agents orchestrate work, skills define knowledge, and tools provide capabilities. But the output of that work often needs a dedicated interface — a deal dashboard, a listing content studio, a transaction tracker, a lead pipeline. Right now, those interfaces either live outside Nebo (in a browser, in a SaaS tool) or they don't exist at all.

This creates three gaps:

- **Users leave Nebo** to interact with the outputs their agents produce. Every context switch is a leakage point.
- **Developers are constrained** to conversational interfaces for agent interaction. Many workflows need spatial, visual, or data-dense UIs that chat can't provide.
- **The marketplace is limited.** NeboLoop can distribute agent configurations, skills, and plugins — but not full applications. This caps the ceiling on what developers can build and sell.

Meanwhile, users are already running web-based tools alongside Nebo. Every one of those browser tabs is a distribution opportunity we're not capturing.

---

## 3. Proposed Solution

Allow static web applications to be packaged, distributed through NeboLoop, and launched as independent Tauri windows within the Nebo desktop application. Each app window runs in its own webview process with a defined IPC channel back to the Nebo core runtime.

### 3.1 What Is an App Window?

- A standalone Tauri webview window, spawned by Nebo, running a static web app bundle.
- Has its own title bar, taskbar/dock presence, and resize behavior. Users perceive it as a real application, not a tab.
- Communicates with Nebo's Rust backend via Tauri's IPC bridge, scoped by install code.
- Can invoke Nebo primitives (agents, tools, skills) within its declared permission set.
- Network requests are proxied through the Rust backend, bypassing browser sandbox limitations — no CORS, full local network access, full WebSocket support.

### 3.2 What It Is Not

- Not an embedded iframe or tab inside the Nebo main window.
- Not a general-purpose browser or Electron replacement.
- Not a way to run arbitrary server-side code on the user's machine.
- Not a container or VM. The app is client-side JavaScript running in a webview, same as any Tauri frontend.

---

## 4. Architecture

### 4.1 Window Lifecycle

The lifecycle of an app window follows the existing install-code primitive pattern:

1. **Install.** User installs an app from NeboLoop via a structured install code (e.g., `APP-XXXX-XXXX-XXXX`). The app bundle — a ZIP archive containing static assets — is downloaded and stored locally alongside the app's permission manifest.
2. **Launch.** User or agent triggers the app. Nebo spawns a new Tauri webview window, loads the app's `index.html`, and establishes a scoped IPC channel identified by the install code.
3. **Run.** The app operates independently. It can call Nebo APIs (agents, tools, Janus) through the IPC bridge and make external network requests through the Rust proxy layer.
4. **Close.** The window is destroyed. Ephemeral state is lost. Persistent state (if the app uses it) lives in a scoped local store managed by Nebo.
5. **Update.** NeboLoop pushes a new bundle version. Nebo downloads and replaces the local copy. Next launch uses the new version.

### 4.2 Process Model

Each app window is a separate webview process. This gives us:

- **Crash isolation.** One app crashes; Nebo and all other apps keep running.
- **Memory isolation.** No shared DOM, no cross-app state leakage.
- **Security boundary.** The window *is* the sandbox. Origin wall enforcement is structural, not policy-based.

The Rust backend serves as the single point of control. All IPC calls, network requests, and agent invocations pass through it, giving Nebo full observability and policy enforcement over app behavior.

### 4.3 IPC Bridge

The IPC bridge is the app's interface to Nebo. It is the only channel between the app's webview and the Nebo runtime.

**Proposed API surface (conceptual):**

```
nebo.agents.invoke(agentId, message)    → Promise<Response>
nebo.agents.subscribe(agentId, handler) → Unsubscribe
nebo.tools.call(toolId, params)         → Promise<Result>
nebo.janus.complete(prompt, opts)       → Promise<Completion>
nebo.storage.get(key)                   → Promise<Value>
nebo.storage.set(key, value)            → Promise<void>
nebo.events.emit(event, payload)        → void
nebo.events.on(event, handler)          → Unsubscribe
nebo.http.request(url, opts)            → Promise<Response>
nebo.ws.connect(url, opts)              → WebSocket
```

All calls are asynchronous. All calls are permission-gated. The Rust backend validates every invocation against the app's declared manifest before execution.

### 4.4 Network Proxy Layer

App network requests (`nebo.http.request`, `nebo.ws.connect`) are routed through Nebo's Rust backend, not the webview's native fetch. This provides:

- **No CORS restrictions.** The Rust layer makes the actual HTTP call; the browser sandbox is never involved.
- **Local network access.** Apps can reach `localhost`, LAN devices, IoT endpoints — things browser tabs cannot.
- **Full WebSocket support.** Persistent connections without browser limitations on concurrent sockets.
- **Observability.** Every outbound request can be logged, metered, and policy-checked by the Nebo runtime.
- **Transparent Janus routing.** If an app makes an LLM API call (e.g., to OpenAI), Janus can intercept and route it optimally — the app gets model routing, cost optimization, and usage analytics without changing its code.

### 4.5 Event Bus

Apps communicate with the broader Nebo ecosystem through a lightweight event bus:

- **App → Nebo:** The app emits named events (e.g., `deal-evaluated`, `listing-drafted`, `lead-qualified`). Nebo agents can subscribe and react.
- **Nebo → App:** Agents or the Nebo runtime can push events to an app window (e.g., `new-data-available`, `agent-completed`). The app handles them via `nebo.events.on`.
- **App → App:** Not directly. If two apps need to coordinate, they do so through a shared agent or through Nebo's event bus with the runtime as intermediary.

This keeps apps decoupled while letting the agent layer orchestrate across multiple apps when needed.

---

## 5. Permission Model

### 5.1 Design Principle

Permissions are declared at install time, not requested at runtime. The install code carries the permission manifest. The user reviews and approves the full permission set once, at installation. This mirrors the curated app store model Nebo already uses for skills and plugins.

### 5.2 Permission Categories

| Category | Examples | Sensitivity |
|---|---|---|
| **Agent Access** | Invoke specific agents, subscribe to agent events | Medium |
| **Tool Access** | Call specific tools (filesystem, browser, etc.) | High |
| **Janus Access** | Make LLM completions, access model routing | Medium |
| **Network — Declared Domains** | HTTP/WS to explicitly listed external domains | Medium |
| **Network — Local** | Access to localhost, LAN, mDNS | High |
| **Network — Unrestricted** | Any outbound HTTP/WS | Critical |
| **Storage** | Read/write to scoped local key-value store | Low |
| **Events** | Emit and subscribe to named events | Low |
| **System** | Window management, notifications, clipboard | Medium |

### 5.3 Manifest Structure (Conceptual)

```yaml
id: APP-REAL-ESTX-DASH
name: Real Estate Deal Dashboard
version: 1.0.0
permissions:
  agents:
    - real-estate-analyst
    - listing-content-writer
  tools:
    - filesystem:read(~/Documents/deals/*)
  janus: true
  network:
    domains:
      - api.zillow.com
      - maps.googleapis.com
    local: false
  storage: true
  events:
    emit:
      - deal-evaluated
      - listing-drafted
    subscribe:
      - new-lead
      - market-update
```

### 5.4 Open Question: Runtime Prompting

Should apps be able to request *additional* permissions at runtime (with user approval), or is the install manifest the hard ceiling? Arguments both ways:

- **Manifest-only** is simpler, more predictable, and easier to audit. Users know exactly what they approved. Developers declare everything upfront.
- **Runtime prompting** is more flexible. An app might not know it needs local network access until the user configures a local device. But it introduces prompt fatigue and a larger attack surface.

Current lean: manifest-only for v1. Revisit if developer feedback demands it.

---

## 6. Storage & State

### 6.1 Scoped Local Storage

Each app gets a private key-value store, managed by Nebo's Rust backend, scoped by install code. The app accesses it through `nebo.storage.get/set`. Data persists across window closes and app updates.

This is intentionally simple. Apps that need richer data models (relational queries, full-text search) should use an agent or tool that provides those capabilities, keeping the app layer thin.

### 6.2 Shared State via Agents

Apps don't share storage directly. If two apps need access to the same data (e.g., a deal pipeline), they both interact with a shared agent that owns that data. The agent is the single source of truth. This avoids the coordination problems of shared mutable state and keeps the agent layer as the canonical data authority.

---

## 7. Janus Integration

### 7.1 Direct LLM Access

Apps can call `nebo.janus.complete()` to make LLM completions through Janus. This gives every app in the ecosystem:

- Automatic model routing (Janus selects the optimal model based on task, cost, and availability).
- Usage tracking and cost attribution per app.
- Rate limiting and quota enforcement through the user's Janus balance.

### 7.2 Transparent Interception

If an app makes a raw HTTP call to a known LLM provider endpoint (e.g., `api.openai.com/v1/chat/completions`), the Rust proxy layer can optionally intercept and reroute through Janus. The app doesn't need to know about Janus at all — it just gets better routing, lower cost, and full observability for free.

This is a significant developer acquisition lever: "Port your existing web app to Nebo. Change nothing. Get automatic model optimization."

### 7.3 Open Question: Billing

Does the app's Janus usage count against the user's balance, or does the app developer subsidize it? Or a split? This has marketplace pricing implications and needs to be resolved before any paid apps ship.

---

## 8. Security Considerations

### 8.1 Origin Wall Integration

App windows are a new origin type in Nebo's existing origin wall system. The tag is `origin:app:{install-code}`. Default policy is deny-all; the permission manifest defines explicit allows.

### 8.2 Threat Surface

| Threat | Mitigation |
|---|---|
| Malicious app exfiltrates user data | Network permissions are domain-scoped. Unrestricted network access requires explicit Critical-level approval. All outbound requests are logged. |
| App invokes dangerous tools | Tool access is declared in manifest. The Rust backend validates every tool call against the manifest. |
| App escalates privileges via agent | Agent invocations are scoped by manifest. Agents themselves enforce the origin wall — they know the call came from an app, not a user. |
| Cross-app contamination | Process isolation. No shared DOM, no shared storage, no direct app-to-app communication. |
| Supply chain attack via app update | NeboLoop is the signing authority. Updates are signed and verified. Bundle integrity is checked at launch. |
| App persists malicious state | Storage is scoped and inspectable. Users can clear any app's storage. Uninstall removes all associated data. |

### 8.3 NeboLoop Review

All apps distributed through NeboLoop undergo the same curation process as skills and plugins. The signing authority model already exists. Apps with Critical-level permissions (unrestricted network, system access) receive elevated review.

---

## 9. Developer Experience

### 9.1 What Developers Ship

A static web app bundle: HTML, CSS, JavaScript, and assets. No server component. No custom runtime. Any framework works — React, Svelte, Vue, vanilla JS. If it builds to static files, it runs in Nebo.

### 9.2 Nebo SDK

A lightweight JavaScript SDK (`@nebo/app-sdk`) that wraps the Tauri IPC bridge with typed interfaces:

```javascript
import { agents, tools, janus, storage, events, http } from '@nebo/app-sdk';

// Invoke an agent
const analysis = await agents.invoke('real-estate-analyst', {
  message: 'Evaluate this deal',
  data: dealObject
});

// Make an LLM call through Janus
const summary = await janus.complete({
  prompt: 'Summarize these comps...',
  model: 'auto'  // Janus picks the best model
});

// Emit an event for other agents to consume
events.emit('deal-evaluated', { dealId: '123', score: 87 });
```

### 9.3 Local Development

Developers run their app in a browser during development (the SDK provides a mock/stub layer for `nebo.*` APIs) and package it for Nebo distribution when ready. No special build tooling required beyond what the framework already provides.

### 9.4 Marketplace Monetization

App developers can publish free or paid apps on NeboLoop. Nebo's existing marketplace infrastructure handles licensing, payments, and distribution. Apps that use Janus represent a recurring revenue component beyond the one-time purchase — every LLM call the app makes flows through the user's Janus balance.

---

## 10. Marketplace & Distribution Impact

### 10.1 What Changes

NeboLoop currently distributes four artifact types: Roles (Agents), Skills, Extensions, and Integrations. App Windows add a fifth: **Applications**.

This is a qualitative shift. Skills and agents are building blocks. Applications are finished products. This moves NeboLoop from a component marketplace to an app store — while retaining the component marketplace underneath.

### 10.2 Vertical Packaging

An app can bundle agents, skills, and tools into a single installable unit. The real estate agent package, for example, becomes:

- **Real Estate Deal Dashboard** (App Window) — visual deal pipeline, comp analysis, scoring UI.
- **Real Estate Analyst** (Agent) — the intelligence layer, invoked by the app.
- **Real Estate Skills** (Skills) — market knowledge, evaluation frameworks.
- **MLS/Zillow Connector** (Tool) — data access.

One install code. One purchase. The user gets an agent-powered application, not a collection of parts they have to assemble.

### 10.3 Competitive Positioning

This positions Nebo as an **AI-native app runtime**, not just an agent platform. The competitive frame shifts:

- vs. ChatGPT/other chat AI: "Nebo runs real applications, not just conversations."
- vs. Electron/Tauri standalone apps: "Build your app once, distribute through NeboLoop, get AI superpowers and model routing for free."
- vs. browser-based SaaS: "Nebo apps have no CORS, local network access, native window presence, and an agent backbone."

For the acquisition narrative, this expands Nebo's addressable surface from "AI agent platform" to "AI-native desktop operating environment" — a meaningfully larger story for potential acquirers.

---

## 11. First Use Cases

### 11.1 Real Estate Deal Dashboard (Internal)

The first app window, built internally as a reference implementation:

- Visual deal pipeline with drag-and-drop stages.
- Comp analysis UI powered by the real estate analyst agent.
- Listing content generation (descriptions, social posts, email campaigns) via the listing content writer agent.
- Transaction coordination timeline.
- All data sourced through agents; the app is a pure presentation layer with AI orchestration.

### 11.2 Lead Pipeline Manager

- Ingests leads from the Vertical Acquisition Agent's Reddit/LinkedIn outreach.
- Shows lead status, qualification scores, conversation history.
- One-click actions: "Draft follow-up," "Schedule meeting," "Mark qualified."
- Each action invokes an agent through the IPC bridge.

### 11.3 Janus Control Panel

- Real-time Janus metrics, provider status, model routing visualization.
- Usage analytics, cost breakdown by app/agent/user.
- Built as an app window eating its own dog food.

---

## 12. Open Questions

| # | Question | Notes |
|---|---|---|
| 1 | **Runtime permissions vs. manifest-only?** | Leaning manifest-only for v1. See Section 5.4. |
| 2 | **Janus billing for app LLM usage?** | User balance, developer subsidy, or split? See Section 7.3. |
| 3 | **App update strategy?** | Auto-update on launch, or explicit user approval? Implications for breaking changes. |
| 4 | **Hot reload during development?** | Can we support live reload in a dev-mode window, or does the developer rebuild and relaunch? |
| 5 | **Offline capability?** | Apps are static bundles, so they load offline. But agent/Janus/network calls require connectivity. How should apps handle degraded mode? |
| 6 | **App-to-app communication?** | Currently disallowed (must go through agents). Is there a valid use case for direct IPC between app windows? |
| 7 | **Window management API?** | Should apps be able to spawn child windows, control their own dimensions, request fullscreen? How much window management do we expose? |
| 8 | **Patent implications?** | Does the install-code-as-permission-manifest pattern extend the Conversational Control Plane patent (No. 63/991,806), or does it warrant a separate filing? |
| 9 | **Mobile parity?** | If Nebo expands to mobile, do app windows become embedded views? How much of this architecture is desktop-only? |

---

## 13. Success Criteria

If we move forward, we'd validate this capability against the following:

- **Developer adoption:** At least 3 non-trivial apps built (1 internal, 2 external) within 90 days of SDK availability.
- **User engagement:** App window sessions represent >30% of total Nebo usage time within 6 months.
- **Marketplace impact:** Applications become the highest-revenue category in NeboLoop within 12 months.
- **Architectural soundness:** Zero security incidents attributable to the app window sandbox in the first year.

---

## 14. Recommendation

Move to a full specification. The architectural fit is strong — Tauri's webview model, the install-code primitive, the origin wall, and the Janus routing layer all align naturally with this capability. The marketplace impact is potentially transformative. The risk is manageable given the process isolation model and the existing security framework.

Suggested next steps:

1. Build a minimal proof-of-concept: a single app window (the Janus Control Panel) with agent invocation and Janus direct access. Validate the IPC bridge and proxy layer.
2. Define the SDK API surface and publish a draft developer guide.
3. Ship the real estate deal dashboard as the first marketplace app, bundled with the real estate agent package.
4. Open the SDK to external developers and begin marketplace onboarding.

---

*This document is an exploratory draft. It is intended to frame the opportunity and architecture, not to serve as a implementation specification. A full PRD with detailed technical specifications, timeline, and resource requirements will follow if the decision is made to proceed.*
