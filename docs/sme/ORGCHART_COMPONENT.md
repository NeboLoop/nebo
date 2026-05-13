# OrgChart Component — Frontend SME Reference

> Interactive, pannable/zoomable org chart for visualizing agent hierarchies.
> Layout engine ported from Paperclip (MIT). Adapted for Svelte 5 + DaisyUI.

**Last verified against source:** 2026-04-27

---

## Table of Contents

1. [Overview](#1-overview)
2. [File Locations](#2-file-locations)
3. [Data Shape](#3-data-shape)
4. [Basic Usage](#4-basic-usage)
5. [Props Reference](#5-props-reference)
6. [Building the Tree from Flat Agent Data](#6-building-the-tree-from-flat-agent-data)
7. [Layout Engine API](#7-layout-engine-api)
8. [Interaction Model](#8-interaction-model)
9. [Styling & Theming](#9-styling--theming)
10. [Customization Guide](#10-customization-guide)
11. [Attribution](#11-attribution)

---

## 1. Overview

The OrgChart component renders a hierarchical tree of agents as positioned cards
with connector lines. It supports mouse drag panning, scroll-wheel zoom,
pinch-to-zoom on touch devices, and a fit-to-screen control.

The architecture is split into two layers:

- **`orgChartLayout.ts`** — Pure TypeScript layout engine. No framework deps.
  Computes x/y positions, edges, bounds. Unit-testable in isolation.
- **`OrgChart.svelte`** — Svelte 5 component that consumes the layout engine
  and renders interactive SVG connectors + HTML cards.

---

## 2. File Locations

```
app/src/
├── lib/
│   ├── utils/
│   │   └── orgChartLayout.ts      # Layout engine (pure math)
│   └── components/
│       └── agent/
│           └── OrgChart.svelte    # Interactive component
```

---

## 3. Data Shape

The component expects an array of `OrgNode` trees:

```typescript
interface OrgNode {
  id: string;        // Unique agent identifier
  name: string;      // Display name
  role: string;      // Role label (e.g. "CEO", "Engineer")
  status: string;    // One of: running, active, paused, idle, error, terminated
  reports: OrgNode[];// Direct reports (recursive children)
  icon?: string;     // Optional icon identifier
  subtitle?: string; // Optional secondary text (capabilities, adapter type, etc.)
}
```

Each root in the array is rendered as a separate tree. Multiple roots are laid
out side by side.

---

## 4. Basic Usage

```svelte
<script lang="ts">
  import OrgChart from '$lib/components/agent/OrgChart.svelte';
  import { goto } from '$app/navigation';
  import type { OrgNode } from '$lib/utils/orgChartLayout';

  const agents: OrgNode[] = [
    {
      id: 'ceo-1',
      name: 'Atlas',
      role: 'CEO',
      status: 'active',
      reports: [
        {
          id: 'cto-1',
          name: 'Nova',
          role: 'CTO',
          status: 'active',
          subtitle: 'Engineering & Architecture',
          reports: [
            { id: 'eng-1', name: 'Coder', role: 'Engineer', status: 'running', reports: [] },
            { id: 'eng-2', name: 'Tester', role: 'QA', status: 'idle', reports: [] }
          ]
        },
        {
          id: 'cmo-1',
          name: 'Pixel',
          role: 'CMO',
          status: 'paused',
          reports: []
        }
      ]
    }
  ];
</script>

<div class="h-[600px]">
  <OrgChart nodes={agents} onNodeClick={(node) => goto(`/agents/${node.id}`)} />
</div>
```

> **Important:** The component fills its parent container. Wrap it in a
> div with an explicit height (e.g. `h-[600px]`, `h-full`, `h-screen`).

---

## 5. Props Reference

| Prop | Type | Default | Description |
|------|------|---------|-------------|
| `nodes` | `OrgNode[]` | `[]` | Tree data to render |
| `loading` | `boolean` | `false` | Shows a DaisyUI spinner when true |
| `onNodeClick` | `(node: LayoutNode) => void` | `undefined` | Called when a card is clicked |

---

## 6. Building the Tree from Flat Agent Data

If your agent data comes as a flat array with `parentId` references, convert it
to the tree structure before passing it in:

```typescript
import type { OrgNode } from '$lib/utils/orgChartLayout';

interface FlatAgent {
  id: string;
  name: string;
  role: string;
  status: string;
  parentId?: string | null;
}

function buildOrgTree(agents: FlatAgent[]): OrgNode[] {
  const map = new Map<string, OrgNode>();

  // Create nodes
  for (const a of agents) {
    map.set(a.id, { id: a.id, name: a.name, role: a.role, status: a.status, reports: [] });
  }

  // Wire parent→child
  const roots: OrgNode[] = [];
  for (const a of agents) {
    const node = map.get(a.id)!;
    if (a.parentId && map.has(a.parentId)) {
      map.get(a.parentId)!.reports.push(node);
    } else {
      roots.push(node);
    }
  }

  return roots;
}
```

---

## 7. Layout Engine API

All exports from `$lib/utils/orgChartLayout.ts`:

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `CARD_W` | 200 | Card width in px |
| `CARD_H` | 100 | Card height in px |
| `GAP_X` | 32 | Horizontal gap between sibling subtrees |
| `GAP_Y` | 80 | Vertical gap between levels |
| `PADDING` | 60 | Canvas padding |
| `MIN_ZOOM` | 0.2 | Minimum zoom level |
| `MAX_ZOOM` | 2 | Maximum zoom level |

### Functions

**`layoutForest(roots: OrgNode[]): LayoutNode[]`**
Main entry point. Takes an array of root nodes, returns positioned `LayoutNode`
trees with `x` and `y` coordinates assigned.

**`flattenLayout(nodes: LayoutNode[]): LayoutNode[]`**
Flattens the positioned tree into a flat array for iteration/rendering.

**`collectEdges(nodes: LayoutNode[]): Edge[]`**
Returns all parent→child edges as `{ parent: LayoutNode, child: LayoutNode }`.

**`computeBounds(allNodes: LayoutNode[]): Bounds`**
Returns `{ width, height }` of the full chart canvas.

**`clampZoom(value: number): number`**
Clamps a zoom level to `[MIN_ZOOM, MAX_ZOOM]`.

**`fitToContainer(bounds, containerW, containerH): { zoom, pan }`**
Calculates the zoom and pan values to fit the chart within a container.

**`edgePath(edge: Edge): string`**
Returns an SVG path string for an orthogonal elbow connector.

---

## 8. Interaction Model

| Gesture | Action |
|---------|--------|
| Mouse drag (on background) | Pan the canvas |
| Scroll wheel | Zoom toward cursor |
| Click on card | Fires `onNodeClick` |
| Touch drag (single finger) | Pan |
| Pinch (two fingers) | Zoom toward pinch center |
| `+` button | Zoom in toward center |
| `−` button | Zoom out from center |
| Fit button | Auto-fit entire chart to viewport |

Touch gestures include a movement threshold (`TOUCH_MOVE_THRESHOLD = 6px`) to
distinguish taps from drags. After a drag/pinch gesture, card clicks are
suppressed for 400ms to prevent accidental navigation.

---

## 9. Styling & Theming

The component uses DaisyUI semantic classes throughout, so it automatically
follows whatever DaisyUI theme is active:

- **Cards:** `bg-base-100`, `border-base-300`
- **Background:** `bg-base-200/30`
- **Text:** `text-base-content` with opacity variants
- **Avatar circle:** `bg-base-300`
- **Connectors:** `oklch(var(--bc) / 0.2)` (base-content at 20% opacity)

Status dot colors are hardcoded hex values that work across light/dark themes:
- `running` → cyan `#22d3ee`
- `active` → green `#4ade80`
- `paused` / `idle` → yellow `#facc15`
- `error` → red `#f87171`
- `terminated` → gray `#a3a3a3`

---

## 10. Customization Guide

### Changing card dimensions

Update the constants in `orgChartLayout.ts`. Both the layout engine and the
component reference them from the same source:

```typescript
export const CARD_W = 240;  // wider cards
export const CARD_H = 120;  // taller cards
export const GAP_Y = 100;   // more vertical spacing
```

### Adding fields to cards

Edit the card markup in `OrgChart.svelte`. The `LayoutNode` type passes through
any extra fields from `OrgNode`, so add them to the interface in the layout
engine and they'll be available in the template.

### Custom connector styles

The SVG path is generated by `edgePath()`. For curved connectors instead of
orthogonal elbows, replace the path logic:

```typescript
// Curved bezier instead of elbows
export function edgePath(edge: Edge): string {
  const x1 = edge.parent.x + CARD_W / 2;
  const y1 = edge.parent.y + CARD_H;
  const x2 = edge.child.x + CARD_W / 2;
  const y2 = edge.child.y;
  const midY = (y1 + y2) / 2;
  return `M ${x1} ${y1} C ${x1} ${midY}, ${x2} ${midY}, ${x2} ${y2}`;
}
```

### Using with real agent data from Nebo API

Wire up the component in a route page with your agent API:

```svelte
<script lang="ts">
  import OrgChart from '$lib/components/agent/OrgChart.svelte';
  import { goto } from '$app/navigation';
  // import your agent store/API here
  // import { agents } from '$lib/stores';

  // Convert flat agent list → OrgNode tree using buildOrgTree() from §6
</script>

<div class="h-full">
  <OrgChart nodes={agentTree} onNodeClick={(n) => goto(`/agents/${n.id}`)} />
</div>
```

---

## 11. Attribution

Layout algorithm ported from [Paperclip](https://github.com/paperclipai/paperclip)
(MIT License). Original implementation: React/TypeScript by @dotta.
Adapted for Svelte 5 + DaisyUI by Nebo team, April 2026.
