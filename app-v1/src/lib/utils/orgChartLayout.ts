/**
 * Org Chart Layout Engine
 * 
 * Pure layout algorithm for hierarchical org charts.
 * Ported from Paperclip (MIT) — framework-agnostic tree positioning math.
 */

// ── Layout constants ──────────────────────────────────────────────────

export const CARD_W = 200;
export const CARD_H = 100;
export const GAP_X = 32;
export const GAP_Y = 80;
export const PADDING = 60;
export const MIN_ZOOM = 0.2;
export const MAX_ZOOM = 2;
export const TOUCH_MOVE_THRESHOLD = 6;

// ── Types ─────────────────────────────────────────────────────────────

/** Input node from the data source — recursive tree structure. */
export interface OrgNode {
	id: string;
	name: string;
	role: string;
	status: string;
	reports: OrgNode[];
	/** Optional icon identifier */
	icon?: string;
	/** Optional subtitle / capabilities */
	subtitle?: string;
}

/** Positioned node after layout — has x,y coords + children. */
export interface LayoutNode {
	id: string;
	name: string;
	role: string;
	status: string;
	x: number;
	y: number;
	children: LayoutNode[];
	icon?: string;
	subtitle?: string;
}

export interface Edge {
	parent: LayoutNode;
	child: LayoutNode;
}

export interface Bounds {
	width: number;
	height: number;
}

export interface Point {
	x: number;
	y: number;
}

// ── Layout algorithm ──────────────────────────────────────────────────

/** Compute the width each subtree needs. */
function subtreeWidth(node: OrgNode): number {
	if (node.reports.length === 0) return CARD_W;
	const childrenW = node.reports.reduce((sum, c) => sum + subtreeWidth(c), 0);
	const gaps = (node.reports.length - 1) * GAP_X;
	return Math.max(CARD_W, childrenW + gaps);
}

/** Recursively assign x,y positions. */
function layoutTree(node: OrgNode, x: number, y: number): LayoutNode {
	const totalW = subtreeWidth(node);
	const layoutChildren: LayoutNode[] = [];

	if (node.reports.length > 0) {
		const childrenW = node.reports.reduce((sum, c) => sum + subtreeWidth(c), 0);
		const gaps = (node.reports.length - 1) * GAP_X;
		let cx = x + (totalW - childrenW - gaps) / 2;

		for (const child of node.reports) {
			const cw = subtreeWidth(child);
			layoutChildren.push(layoutTree(child, cx, y + CARD_H + GAP_Y));
			cx += cw + GAP_X;
		}
	}

	return {
		id: node.id,
		name: node.name,
		role: node.role,
		status: node.status,
		x: x + (totalW - CARD_W) / 2,
		y,
		children: layoutChildren,
		icon: node.icon,
		subtitle: node.subtitle
	};
}

/** Layout all root nodes side by side. */
export function layoutForest(roots: OrgNode[]): LayoutNode[] {
	if (roots.length === 0) return [];

	let x = PADDING;
	const y = PADDING;

	const result: LayoutNode[] = [];
	for (const root of roots) {
		const w = subtreeWidth(root);
		result.push(layoutTree(root, x, y));
		x += w + GAP_X;
	}

	return result;
}

/** Flatten layout tree to a list of nodes. */
export function flattenLayout(nodes: LayoutNode[]): LayoutNode[] {
	const result: LayoutNode[] = [];
	function walk(n: LayoutNode) {
		result.push(n);
		n.children.forEach(walk);
	}
	nodes.forEach(walk);
	return result;
}

/** Collect all parent→child edges. */
export function collectEdges(nodes: LayoutNode[]): Edge[] {
	const edges: Edge[] = [];
	function walk(n: LayoutNode) {
		for (const c of n.children) {
			edges.push({ parent: n, child: c });
			walk(c);
		}
	}
	nodes.forEach(walk);
	return edges;
}

/** Compute bounding box of all positioned nodes. */
export function computeBounds(allNodes: LayoutNode[]): Bounds {
	if (allNodes.length === 0) return { width: 800, height: 600 };
	let maxX = 0;
	let maxY = 0;
	for (const n of allNodes) {
		maxX = Math.max(maxX, n.x + CARD_W);
		maxY = Math.max(maxY, n.y + CARD_H);
	}
	return { width: maxX + PADDING, height: maxY + PADDING };
}

/** Clamp zoom to allowed range. */
export function clampZoom(value: number): number {
	return Math.min(Math.max(value, MIN_ZOOM), MAX_ZOOM);
}

/** Calculate fit-to-screen zoom and pan. */
export function fitToContainer(
	bounds: Bounds,
	containerW: number,
	containerH: number
): { zoom: number; pan: Point } {
	const scaleX = (containerW - 40) / bounds.width;
	const scaleY = (containerH - 40) / bounds.height;
	const zoom = Math.min(scaleX, scaleY, 1);
	const chartW = bounds.width * zoom;
	const chartH = bounds.height * zoom;
	return {
		zoom,
		pan: {
			x: (containerW - chartW) / 2,
			y: (containerH - chartH) / 2
		}
	};
}

// ── SVG edge path helper ──────────────────────────────────────────────

/** Generate an orthogonal elbow connector path string. */
export function edgePath(edge: Edge): string {
	const x1 = edge.parent.x + CARD_W / 2;
	const y1 = edge.parent.y + CARD_H;
	const x2 = edge.child.x + CARD_W / 2;
	const y2 = edge.child.y;
	const midY = (y1 + y2) / 2;
	return `M ${x1} ${y1} L ${x1} ${midY} L ${x2} ${midY} L ${x2} ${y2}`;
}
