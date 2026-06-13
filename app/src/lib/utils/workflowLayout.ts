/**
 * Workflow Canvas Layout Engine
 *
 * Positions workflow nodes as a directed graph (DAG) using dagre (Sugiyama algorithm).
 * Supports linear chains, branching (conditions/loops), and merge points.
 *
 * Data model:
 * - `activities[]` defines the set of nodes
 * - `connections[]` defines edges between nodes (optional — falls back to array order)
 * - Trigger and emit are implicit bookend nodes
 */

import dagre from '@dagrejs/dagre';
import { isBranchingType, getActivityType } from '$lib/utils/workflowTypes';

// ── Layout constants ──────────────────────────────────────────────────
export const NODE_W = 240;
export const NODE_H = 88;
export const TRIGGER_W = 140;
export const TRIGGER_H = 56;
export const GAP_X = 60;
export const GAP_Y = 24;
export const PADDING = 40;
export const MIN_ZOOM = 0.3;
export const MAX_ZOOM = 2;

// ── Types ─────────────────────────────────────────────────────────────

export interface WorkflowConnection {
  from: string;  // node id (or '__trigger__')
  to: string;    // node id (or '__emit__')
  label?: string; // branch label (e.g. 'True', 'False', 'Each item', 'Done')
}

export interface WorkflowNodeData {
  id: string;
  type: 'trigger' | 'activity' | 'emit';
  activityType?: string;
  label: string;
  sublabel?: string;
  skills?: string[];
  stepCount?: number;
  status?: 'success' | 'failed' | 'running' | 'idle';
}

export interface LayoutWorkflowNode extends WorkflowNodeData {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface WorkflowEdge {
  from: LayoutWorkflowNode;
  to: LayoutWorkflowNode;
  label?: string;
}

export type Activity = {
  id: string;
  intent?: string;
  skills?: string[];
  steps?: string[];
  type?: string;
  params?: Record<string, any>;
};

// ── Layout algorithm ──────────────────────────────────────────────────

/**
 * Layout workflow as a DAG: trigger → activities → emit.
 *
 * If `connections` is provided, uses them for the graph structure.
 * Otherwise, falls back to linear chain from the activities array order.
 */
export function layoutWorkflow(
  trigger: { type: string; schedule?: string; event?: string },
  activities: Activity[],
  emit?: string,
  lastRunStatus?: string,
  connections?: WorkflowConnection[],
): { nodes: LayoutWorkflowNode[]; edges: WorkflowEdge[] } {
  // Build adjacency from connections or linear fallback
  const { adj, edgeLabels } = buildAdjacency(activities, emit, connections);

  // Build node data map (not yet positioned)
  const nodeMap = new Map<string, LayoutWorkflowNode>();

  // Trigger node — build descriptive sublabel
  let triggerLabel = 'Manual';
  if (trigger.type === 'schedule') {
    triggerLabel = trigger.schedule || 'Schedule';
  } else if (trigger.type === 'event') {
    triggerLabel = trigger.event || 'Event';
  } else if (trigger.type === 'heartbeat') {
    const interval = (trigger as any).interval || '30m';
    const window = (trigger as any).window;
    triggerLabel = `Every ${interval}`;
    if (window?.start && window?.end) {
      triggerLabel += `, ${window.start}–${window.end}`;
    }
  }

  const triggerNode: LayoutWorkflowNode = {
    id: '__trigger__',
    type: 'trigger',
    label: trigger.type.charAt(0).toUpperCase() + trigger.type.slice(1),
    sublabel: triggerLabel,
    x: 0, y: 0,
    w: TRIGGER_W,
    h: TRIGGER_H,
  };
  nodeMap.set('__trigger__', triggerNode);

  // Activity nodes
  for (const act of activities) {
    const actNode: LayoutWorkflowNode = {
      id: act.id,
      type: 'activity',
      activityType: act.type || 'custom',
      label: act.id,
      sublabel: act.intent,
      skills: act.skills,
      stepCount: act.steps?.length ?? 0,
      status: lastRunStatus === 'success' ? 'success' : lastRunStatus === 'failed' ? 'failed' : 'idle',
      x: 0, y: 0,
      w: NODE_W,
      h: NODE_H,
    };
    nodeMap.set(act.id, actNode);
  }

  // Emit node
  if (emit) {
    const emitNode: LayoutWorkflowNode = {
      id: '__emit__',
      type: 'emit',
      label: 'Emit',
      sublabel: emit,
      x: 0, y: 0,
      w: TRIGGER_W,
      h: TRIGGER_H,
    };
    nodeMap.set('__emit__', emitNode);
  }

  // ── Dagre for X positioning (rank assignment + horizontal spacing) ──
  const g = new dagre.graphlib.Graph();
  g.setGraph({
    rankdir: 'LR',
    nodesep: 80,
    ranksep: 128,
    edgesep: 48,
    marginx: PADDING,
    marginy: PADDING,
  });
  g.setDefaultEdgeLabel(() => ({}));

  for (const [id, node] of nodeMap) {
    g.setNode(id, { width: node.w, height: node.h });
  }
  for (const [fromId, toIds] of adj) {
    for (const toId of toIds) {
      if (nodeMap.has(fromId) && nodeMap.has(toId)) {
        g.setEdge(fromId, toId);
      }
    }
  }

  dagre.layout(g);

  // Read X positions from dagre (it handles rank spacing well)
  // Read Y as baseline — we'll override for branching nodes
  for (const [id, node] of nodeMap) {
    const dn = g.node(id);
    if (dn) {
      node.x = dn.x - node.w / 2;
      node.y = dn.y - node.h / 2;
    }
  }

  // ── Override Y for forking nodes: spread children vertically around parent ──
  const branchOffset = NODE_H + GAP_Y; // vertical distance between branch centers

  // Build reverse map (child → parents)
  const parentMap = new Map<string, string[]>();
  for (const [fromId, toIds] of adj) {
    for (const toId of toIds) {
      if (!parentMap.has(toId)) parentMap.set(toId, []);
      parentMap.get(toId)!.push(fromId);
    }
  }

  // Collect exclusive subtree via DFS (stops at merge points)
  function collectSubtree(startId: string, forkParentId: string): Set<string> {
    const visited = new Set<string>();
    const stack = [startId];
    while (stack.length > 0) {
      const id = stack.pop()!;
      if (visited.has(id)) continue;
      visited.add(id);
      for (const child of (adj.get(id) ?? [])) {
        const parents = parentMap.get(child) ?? [];
        const allParentsInSubtree = parents.every(p => visited.has(p) || p === forkParentId);
        if (allParentsInSubtree) {
          stack.push(child);
        }
      }
    }
    return visited;
  }

  // Find all forking nodes: any node with 2+ outgoing edges (branching types OR parallel outputs)
  const allNodeIds = [...nodeMap.keys()];
  const forkNodes: string[] = [];
  for (const nodeId of allNodeIds) {
    const children = adj.get(nodeId) ?? [];
    if (children.length >= 2) forkNodes.push(nodeId);
  }

  // Process forks: spread children vertically around parent center
  for (const forkId of forkNodes) {
    const children = adj.get(forkId) ?? [];
    if (children.length < 2) continue;

    const parentNode = nodeMap.get(forkId)!;
    const parentCenterY = parentNode.y + parentNode.h / 2;
    const isBranching = activities.some(a => a.id === forkId && isBranchingType(a.type));

    // Order children: for condition/loop, True/Each-item first; for parallel, keep edge order
    let orderedChildren: string[];
    if (isBranching) {
      const trueChild = children.find(c => {
        const label = edgeLabels.get(`${forkId}→${c}`);
        return label === 'True' || label === 'Each item';
      });
      if (trueChild) {
        orderedChildren = [trueChild, ...children.filter(c => c !== trueChild)];
      } else {
        orderedChildren = [...children];
      }
    } else {
      orderedChildren = [...children];
    }

    // Spread N children evenly around parent center
    const count = orderedChildren.length;
    for (let i = 0; i < count; i++) {
      const childId = orderedChildren[i];
      const targetCenterY = parentCenterY + (i - (count - 1) / 2) * branchOffset;

      const subtree = collectSubtree(childId, forkId);
      const rootNode = nodeMap.get(childId);
      if (rootNode) {
        const currentCenterY = rootNode.y + rootNode.h / 2;
        const delta = targetCenterY - currentCenterY;
        for (const id of subtree) {
          const n = nodeMap.get(id);
          if (n) n.y += delta;
        }
      }
    }
  }

  // Build edges
  const edges: WorkflowEdge[] = [];
  for (const [fromId, toIds] of adj) {
    const fromNode = nodeMap.get(fromId);
    if (!fromNode) continue;
    for (const toId of toIds) {
      const toNode = nodeMap.get(toId);
      if (!toNode) continue;
      const label = edgeLabels.get(`${fromId}→${toId}`);
      edges.push({ from: fromNode, to: toNode, label });
    }
  }

  const nodes = [...nodeMap.values()];
  return { nodes, edges };
}

/** Build adjacency list from connections or linear fallback */
function buildAdjacency(
  activities: Activity[],
  emit: string | undefined,
  connections?: WorkflowConnection[],
): { adj: Map<string, string[]>; edgeLabels: Map<string, string> } {
  const adj = new Map<string, string[]>();
  const edgeLabels = new Map<string, string>(); // key: "from→to"

  // A non-null connections array is AUTHORITATIVE even when empty —
  // deleting the last edge must not resurrect the linear fallback.
  if (connections) {
    // Use explicit connections
    for (const c of connections) {
      if (!adj.has(c.from)) adj.set(c.from, []);
      adj.get(c.from)!.push(c.to);
      if (c.label) edgeLabels.set(`${c.from}→${c.to}`, c.label);
    }
  } else {
    // Linear fallback: trigger → act[0] → act[1] → ... → emit
    let prev = '__trigger__';
    for (const act of activities) {
      if (!adj.has(prev)) adj.set(prev, []);
      adj.get(prev)!.push(act.id);
      prev = act.id;
    }
    if (emit && prev !== '__trigger__') {
      if (!adj.has(prev)) adj.set(prev, []);
      adj.get(prev)!.push('__emit__');
    }
  }

  return { adj, edgeLabels };
}


// ── Reusable math ──────────────────────────────────────────────────

export function clampZoom(value: number): number {
  return Math.min(Math.max(value, MIN_ZOOM), MAX_ZOOM);
}

export function computeBounds(
  nodes: LayoutWorkflowNode[],
): { width: number; height: number; minX: number; minY: number } {
  if (nodes.length === 0) return { width: 600, height: 300, minX: 0, minY: 0 };
  // Dragged nodes can sit at negative coordinates — bounds must track the
  // true min or fit-to-screen centers wrong and clips content off the top/left.
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of nodes) {
    minX = Math.min(minX, n.x);
    minY = Math.min(minY, n.y);
    maxX = Math.max(maxX, n.x + n.w);
    maxY = Math.max(maxY, n.y + n.h);
  }
  // Content between origin and the nodes still renders, so extend to 0.
  minX = Math.min(minX, 0);
  minY = Math.min(minY, 0);
  return { width: maxX - minX + PADDING, height: maxY - minY + PADDING, minX, minY };
}

export function fitToContainer(
  bounds: { width: number; height: number; minX?: number; minY?: number },
  containerW: number,
  containerH: number,
): { zoom: number; pan: { x: number; y: number } } {
  const scaleX = (containerW - 40) / bounds.width;
  const scaleY = (containerH - 40) / bounds.height;
  const zoom = Math.min(scaleX, scaleY, 1);
  const chartW = bounds.width * zoom;
  const chartH = bounds.height * zoom;
  return {
    zoom,
    pan: {
      // Shift by the (possibly negative) content origin so the top-left of
      // the actual content — not coordinate (0,0) — lands in view.
      x: (containerW - chartW) / 2 - (bounds.minX ?? 0) * zoom,
      y: (containerH - chartH) / 2 - (bounds.minY ?? 0) * zoom,
    },
  };
}

/** Bezier curve between two nodes (right edge → left edge). */
export function edgePath(edge: WorkflowEdge): string {
  const x1 = edge.from.x + edge.from.w;
  const y1 = edge.from.y + edge.from.h / 2;
  const x2 = edge.to.x;
  const y2 = edge.to.y + edge.to.h / 2;
  const cpOffset = Math.min((x2 - x1) * 0.5, 40);
  return `M ${x1} ${y1} C ${x1 + cpOffset} ${y1}, ${x2 - cpOffset} ${y2}, ${x2} ${y2}`;
}

// ── Workflow mutation helpers ─────────────────────────────────────

/** Smallest `{base}-{n}` id not already taken. A module counter resets on
 *  every page load — saved ids like "a-copy-1" would collide next session. */
function uniqueNodeId(activities: Activity[], base: string): string {
  const taken = new Set(activities.map(a => a.id));
  let n = 1;
  while (taken.has(`${base}-${n}`)) n++;
  return `${base}-${n}`;
}

/** Add an activity node to a workflow. Returns a new activities array. */
export function addActivityToWorkflow(
  activities: Activity[],
  afterId: string | null,
  newActivity?: Partial<Activity>,
): Activity[] {
  const act: Activity = {
    id: newActivity?.id || uniqueNodeId(activities, 'activity'),
    intent: newActivity?.intent || 'New activity',
    skills: newActivity?.skills || [],
    steps: newActivity?.steps || [],
    type: newActivity?.type || 'custom',
    params: newActivity?.params || {},
  };
  const result = [...activities];
  if (afterId === null) {
    result.push(act);
  } else if (afterId === '__trigger__') {
    result.splice(0, 0, act);
  } else {
    const idx = result.findIndex(a => a.id === afterId);
    result.splice(idx >= 0 ? idx + 1 : result.length, 0, act);
  }
  return result;
}

/** Remove an activity from a workflow. Returns a new activities array. */
export function removeActivityFromWorkflow(
  activities: Activity[],
  activityId: string,
): Activity[] {
  return activities.filter(a => a.id !== activityId);
}

/** Duplicate an activity in a workflow. Returns a new activities array. */
export function duplicateActivityInWorkflow(
  activities: Activity[],
  activityId: string,
): Activity[] {
  const idx = activities.findIndex(a => a.id === activityId);
  if (idx < 0) return activities;
  const original = activities[idx];
  const dupe: Activity = {
    id: uniqueNodeId(activities, `${original.id}-copy`),
    intent: original.intent,
    skills: original.skills ? [...original.skills] : [],
    steps: original.steps ? [...original.steps] : [],
    type: original.type,
    params: original.params ? { ...original.params } : {},
  };
  const result = [...activities];
  result.splice(idx + 1, 0, dupe);
  return result;
}

/**
 * Add a connection between two nodes.
 * If the workflow has no explicit connections yet, generates them from the activity order first.
 */
export function addConnection(
  connections: WorkflowConnection[] | undefined,
  activities: Activity[],
  emit: string | undefined,
  from: string,
  to: string,
): WorkflowConnection[] {
  // If no explicit connections, generate from linear order
  let conns = connections ? [...connections] : generateLinearConnections(activities, emit);
  conns.push({ from, to });
  return conns;
}

/** Remove a connection. */
export function removeConnection(
  connections: WorkflowConnection[],
  from: string,
  to: string,
): WorkflowConnection[] {
  return connections.filter(c => !(c.from === from && c.to === to));
}

/** Generate linear connections from activity array order. */
export function generateLinearConnections(
  activities: Activity[],
  emit?: string,
): WorkflowConnection[] {
  const conns: WorkflowConnection[] = [];
  let prev = '__trigger__';
  for (const act of activities) {
    conns.push({ from: prev, to: act.id });
    prev = act.id;
  }
  if (emit && activities.length > 0) {
    conns.push({ from: prev, to: '__emit__' });
  }
  return conns;
}
