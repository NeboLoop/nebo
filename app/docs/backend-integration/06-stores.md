# Stores — Reactive State & Backend Endpoints

All 10 reactive stores in `src/lib/stores/`, their current data sources, and how to connect them to the backend.

## Store Inventory

| Store | File | Persistence | Backend Needed |
|-------|------|-------------|----------------|
| `schedule` | `schedule.ts` | In-memory (computed from mockData) | Yes |
| `marketplace` | `marketplace.ts` | In-memory (writable) | Yes |
| `collections` | `collections.ts` | In-memory (writable) | Yes |
| `notifications` | `notifications.ts` | In-memory (writable) | Yes |
| `permissions` | `permissions.ts` | `localStorage` | Yes |
| `onboarding` | `onboarding.ts` | `localStorage` | Yes |
| `devmode` | `devmode.ts` | `localStorage` | No (client-only) |
| `theme` | `theme.ts` | `localStorage` | Optional |
| `sidebar` | `sidebar.ts` | In-memory | No (UI state) |
| `toast` | `toast.ts` | In-memory (ephemeral) | No (UI state) |

---

## 1. Schedule Store — `stores/schedule.ts`

**Current:** Parses `AGENT_CONFIGS` workflows + `MOCK_WORKFLOW_RUNS` at module load to build `CalendarItem[]`.

### Types
```ts
type EventKind = 'sched' | 'event' | 'user';
type RunStatus = 'success' | 'failed' | 'skipped' | 'running' | 'pending';

interface RunData {
  id: string;
  status: RunStatus;
  actualDuration: string;
  startedAt: string;
  completedAt: string;
  tokens?: { input: number; output: number };
  activities?: { id: string; status: string; duration: string; output?: string; error?: string }[];
}

interface CalendarItem {
  id: string;
  agent: string;          // short ID
  agentFull: string;      // full ID
  kind: EventKind;
  label: string;
  days: number[];         // Mon=1..Sun=7
  hour: number;           // fractional
  dur: number;
  end: number;
  workflowId?: string;
  triggerType: string;
  recurrence?: string;
  run?: RunData;
}
```

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `userScheduleItems` | `writable<CalendarItem[]>` | User-created events |
| `addUserItem(item)` | Function | Create user event |
| `updateUserItem(id, changes)` | Function | Edit user event |
| `removeUserItem(id)` | Function | Delete user event |
| `getAllItems(userItems)` | Function | All items (scheduled + event runs + user) |
| `itemsForWeekday(day, enabled, userItems)` | Function | Filter by day + agent |
| `flattenForDate(day, enabled, userItems)` | Function | Sorted items for calendar render |
| `attachRunData(items)` | Function | Attach run status to scheduled items |
| `getRecentRuns(agentFull, workflowId)` | Function | Recent runs for a workflow |
| `runsPerWeek(agentShort, userItems)` | Function | Count weekly runs per agent |
| `getScheduleAgents(userItems)` | Function | Agents with schedule items |
| `snapTo15(hour)` | Function | Snap to 15-minute increment |
| `parseScheduleString(schedule)` | Function | Parse "8:00 AM daily" strings |

### Backend Migration

Replace the module-level `buildItemsFromConfigs()` and `buildEventRunItems()` calls:

```ts
// BEFORE (module load)
const _scheduledItems = buildItemsFromConfigs();  // reads AGENT_CONFIGS
const _eventRunItems = buildEventRunItems();      // reads MOCK_WORKFLOW_RUNS

// AFTER (async init)
async function loadScheduleData() {
  const agents = await fetch('/api/v1/agents').then(r => r.json());
  const items: CalendarItem[] = [];
  for (const agent of agents.agents) {
    const workflows = await fetch(`/api/v1/agents/${agent.id}/workflows`).then(r => r.json());
    // Parse workflow triggers into CalendarItems (same logic as buildItemsFromConfigs)
    // ...
  }
  return items;
}
```

**User events:** `userScheduleItems` → `GET/POST/PUT/DELETE /api/v1/tasks`

---

## 2. Marketplace Store — `stores/marketplace.ts`

**Current:** Writable store seeded with hardcoded installed items. References `MARKETPLACE_AGENT_DETAILS` for dependency cascade.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `installedItems` | `writable<InstalledItem[]>` | Currently installed items |
| `installedIds` | `derived<Set<string>>` | Quick lookup of installed IDs |
| `installItem(item)` | Function | Install item + cascade dependencies |
| `uninstallItem(id)` | Function | Remove item |

### Backend Migration

```ts
// BEFORE
const initialInstalled: InstalledItem[] = [ /* hardcoded */ ];
export const installedItems = writable<InstalledItem[]>(initialInstalled);

// AFTER
export const installedItems = writable<InstalledItem[]>([]);

export async function loadInstalled() {
  const data = await fetch('/api/v1/plugins').then(r => r.json());
  // Combine with GET /api/v1/extensions for skills
  installedItems.set(data.plugins.map(/* transform */));
}
```

| Action | Endpoint |
|--------|----------|
| Load installed | `GET /api/v1/plugins` + `GET /api/v1/extensions` |
| Install item | `POST /api/v1/store/products/{id}/install` (handles cascade) |
| Uninstall item | `DELETE /api/v1/store/products/{id}/install` |

---

## 3. Collections Store — `stores/collections.ts`

**Current:** Writable store seeded from `MARKETPLACE_COLLECTIONS`.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `collections` | `writable<PrivateCollection[]>` | All collections |
| `createCollection(col)` | Function | Create new collection |
| `deleteCollection(id)` | Function | Delete collection |
| `addItemToCollection(colId, itemId)` | Function | Add item to collection |
| `removeItemFromCollection(colId, itemId)` | Function | Remove item |
| `collectionsForOrg(orgId)` | Function | Derived store filtered by org |

### Backend Migration
- Collections are org-scoped marketplace items
- Backend endpoint TBD — likely part of store/marketplace API or NeboLoop API

---

## 4. Notifications Store — `stores/notifications.ts`

**Current:** Writable store with hardcoded initial notifications.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `notifications` | `writable<Notification[]>` | All notifications |
| `unreadCount` | `derived<number>` | Unread count |
| `markAsRead(id)` | Function | Mark one as read |
| `markAllRead()` | Function | Mark all as read |
| `removeNotification(id)` | Function | Delete notification |

### Backend Migration

```ts
// BEFORE
const initialNotifications: Notification[] = [ /* hardcoded */ ];
export const notifications = writable<Notification[]>(initialNotifications);

// AFTER
export const notifications = writable<Notification[]>([]);

export async function loadNotifications() {
  const data = await fetch('/api/v1/notifications').then(r => r.json());
  notifications.set(data.notifications);
}
```

| Action | Endpoint |
|--------|----------|
| Load | `GET /api/v1/notifications` |
| Unread count | `GET /api/v1/notifications/unread-count` |
| Mark as read | `PUT /api/v1/notifications/{id}/read` |
| Mark all read | `PUT /api/v1/notifications/read-all` |
| Delete | `DELETE /api/v1/notifications/{id}` |
| Real-time updates | WebSocket events |

---

## 5. Permissions Store — `stores/permissions.ts`

**Current:** Persists auto-approved action keys in `localStorage`.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `autoApproved` | `writable<string[]>` | List of always-approved action keys |
| `approveAlways(key)` | Function | Add to auto-approve list |
| `isAutoApproved(key)` | Function | Check if key is auto-approved |

### Backend Migration
- **Read:** `GET /api/v1/user/me/permissions` or `GET /api/v1/agent/settings`
- **Write:** `PUT /api/v1/user/me/permissions`
- Could keep localStorage as cache with backend sync

---

## 6. Onboarding Store — `stores/onboarding.ts`

**Current:** Boolean flag in `localStorage`.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `onboardingComplete` | `writable<boolean>` | Whether setup wizard is done |
| `completeOnboarding()` | Function | Mark complete |

### Backend Migration
- **Read:** `GET /api/v1/setup/status` → `setupComplete` field
- **Write:** `POST /api/v1/setup/complete`

---

## 7. Dev Mode Store — `stores/devmode.ts`

**Current:** Boolean toggle in `localStorage`. Gates advanced settings (Providers, Routing, Secrets).

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `devMode` | `writable<boolean>` | Dev mode enabled |

### Backend Migration
- No backend needed — purely client-side UI preference
- Could optionally store in `user_preferences`

---

## 8. Theme Store — `stores/theme.ts`

**Current:** Theme name in `localStorage`. Sets `data-theme` attribute on `<html>`.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `theme` | `writable<string>` | Active DaisyUI theme name |

### Backend Migration
- Optionally sync with `PUT /api/v1/user/me/preferences` → `theme` field
- Keep localStorage as primary for instant theme switching

---

## 9. Sidebar Store — `stores/sidebar.ts`

**Current:** In-memory per-section collapse state.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `sidebarCollapsedFor(section)` | Function | Returns store for section |
| `sidebarCollapsed` | Store | Default section collapse state |

### Backend Migration
- No backend needed — UI layout state only

---

## 10. Toast Store — `stores/toast.ts`

**Current:** In-memory toast queue with auto-dismiss.

### Exports
| Export | Type | Purpose |
|--------|------|---------|
| `toasts` | `writable<Toast[]>` | Active toast list |
| `addToast(message, type, duration)` | Function | Show toast |
| `removeToast(id)` | Function | Dismiss toast |

### Backend Migration
- No backend needed — ephemeral UI notifications only

---

## Migration Order

1. **Notifications** — needed immediately (NotificationBell in global header)
2. **Schedule** — drives the calendar page
3. **Marketplace** — drives install/uninstall across marketplace
4. **Permissions** — needed for ApprovalModal flow
5. **Onboarding** — needed for first-run experience
6. **Collections** — marketplace feature
7. **Theme** — optional sync
8. **DevMode, Sidebar, Toast** — no backend needed
