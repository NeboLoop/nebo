<script lang="ts">
	import {
		layoutWorkflow,
		clampZoom,
		computeBounds,
		fitToContainer,
		edgePath,
		type LayoutWorkflowNode,
		type WorkflowEdge,
		NODE_W,
		NODE_H,
		GAP_X,
		GAP_Y,
		PADDING,
	} from '$lib/utils/workflowLayout';
	import { getActivityType, isBranchingType } from '$lib/utils/workflowTypes';
	import type { WorkflowConfig } from '$lib/types/agentPage';

	let {
		workflow,
		workflowName = '',
		agentId = '',
		mode = 'view',
		selectedNodeId = null,
		onselect,
		onopenCatalog,
		onremove,
		onduplicate,
		oncreateConnection,
		onremoveConnection,
		ondropNode,
	}: {
		workflow: WorkflowConfig;
		workflowName: string;
		agentId: string;
		mode: 'view' | 'edit';
		selectedNodeId: string | null;
		onselect?: (nodeId: string | null) => void;
		onopenCatalog?: (afterNodeId: string | null, branchLabel?: string) => void;
		onremove?: (nodeId: string) => void;
		onduplicate?: (nodeId: string) => void;
		oncreateConnection?: (fromId: string, toId: string) => void;
		onremoveConnection?: (fromId: string, toId: string) => void;
		ondropNode?: (item: Record<string, unknown>, afterNodeId: string | null) => void;
	} = $props();

	// ── Layout computation (single workflow)
	const baseLayout = $derived.by(() => {
		if (!workflow) return { nodes: [] as LayoutWorkflowNode[], edges: [] as WorkflowEdge[] };
		return layoutWorkflow(
			workflow.trigger || { type: 'manual' },
			workflow.activities || [],
			workflow.emit,
			undefined,
			workflow.connections,
		);
	});

	// ── Position overrides (for dragged nodes) ──
	let posOverrides = $state<Record<string, { x: number; y: number }>>({});

	// Clear overrides when the graph structure changes
	let prevNodeIdKey = '';
	$effect(() => {
		const key = baseLayout.nodes.map(n => n.id).join(',');
		if (prevNodeIdKey && key !== prevNodeIdKey) posOverrides = {};
		prevNodeIdKey = key;
	});

	// Display nodes/edges with position overrides applied
	const displayNodes = $derived.by(() => {
		return baseLayout.nodes.map(n => {
			const ov = posOverrides[n.id];
			return ov ? { ...n, x: ov.x, y: ov.y } : n;
		});
	});

	const displayEdges = $derived.by(() => {
		const nodeById = new Map(displayNodes.map(n => [n.id, n]));
		return baseLayout.edges.map(e => ({
			from: nodeById.get(e.from.id) ?? e.from,
			to: nodeById.get(e.to.id) ?? e.to,
			label: e.label,
		}));
	});

	const bounds = $derived(computeBounds(displayNodes));

	// ── Edge selection
	let selectedEdgeKey = $state<string | null>(null);
	let hoveredEdgeKey = $state<string | null>(null);

	function edgeKey(e: WorkflowEdge): string {
		return `${e.from.id}→${e.to.id}`;
	}

	// ── "+" connector positions
	const plusButtons = $derived.by(() => {
		if (mode !== 'edit') return [];
		const buttons: Array<{ x: number; y: number; afterNodeId: string; branchLabel?: string }> = [];

		const outgoingCount = new Map<string, number>();
		for (const edge of displayEdges) {
			outgoingCount.set(edge.from.id, (outgoingCount.get(edge.from.id) ?? 0) + 1);
		}

		for (const edge of displayEdges) {
			const x1 = edge.from.x + edge.from.w;
			const y1 = edge.from.y + edge.from.h / 2;
			const x2 = edge.to.x;
			const y2 = edge.to.y + edge.to.h / 2;
			buttons.push({
				x: (x1 + x2) / 2,
				y: (y1 + y2) / 2,
				afterNodeId: edge.from.id,
			});
		}

		for (const node of displayNodes) {
			const count = outgoingCount.get(node.id) ?? 0;
			if (count === 0) {
				if (node.type === 'activity' && isBranchingType(node.activityType)) {
					const typeDef = getActivityType(node.activityType);
					const labels = typeDef.branchLabels ?? ['Branch 1', 'Branch 2'];
					const spacing = NODE_H * 0.4;
					for (let i = 0; i < labels.length; i++) {
						const offsetY = (i - (labels.length - 1) / 2) * spacing;
						buttons.push({
							x: node.x + node.w + GAP_X / 2,
							y: node.y + node.h / 2 + offsetY,
							afterNodeId: node.id,
							branchLabel: labels[i],
						});
					}
				} else {
					buttons.push({
						x: node.x + node.w + GAP_X / 2,
						y: node.y + node.h / 2,
						afterNodeId: node.id,
					});
				}
			} else if (node.type === 'activity' && isBranchingType(node.activityType)) {
				const typeDef = getActivityType(node.activityType);
				const labels = typeDef.branchLabels ?? [];
				const usedLabels = new Set(
					displayEdges.filter(e => e.from.id === node.id && e.label).map(e => e.label)
				);
				for (const label of labels) {
					if (!usedLabels.has(label)) {
						buttons.push({
							x: node.x + node.w + GAP_X / 2,
							y: node.y + node.h + GAP_Y / 2,
							afterNodeId: node.id,
							branchLabel: label,
						});
					}
				}
			}
		}
		return buttons;
	});

	// ── Pan & zoom
	let pan = $state({ x: 0, y: 0 });
	let zoom = $state(1);
	let panning = $state(false);
	let container = $state<HTMLDivElement | undefined>(undefined);
	let initialized = false;
	let panStart = { x: 0, y: 0, panX: 0, panY: 0 };

	// ── Context menu (node or edge)
	let contextMenu = $state<{
		x: number; y: number;
		nodeId?: string;
		edgeFrom?: string; edgeTo?: string;
	} | null>(null);

	// ── Node dragging
	let nodeDrag = $state<{
		id: string;
		startMouseX: number; startMouseY: number;
		startNodeX: number; startNodeY: number;
	} | null>(null);
	let hasDragged = $state(false);

	// ── Wire dragging (connection creation)
	let wireDrag = $state<{
		sourceId: string;
		startX: number; startY: number;
		currentX: number; currentY: number;
		hoverNodeId: string | null;
	} | null>(null);

	// ── Catalog drag-over state
	let catalogDragHoverNodeId = $state<string | null>(null);

	function hasCatalogDragType(e: DragEvent): boolean {
		if (!e.dataTransfer) return false;
		return Array.from(e.dataTransfer.types).includes('application/x-workflow-node');
	}

	function onCanvasDragOver(e: DragEvent) {
		if (!hasCatalogDragType(e)) return;
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
		const canvas = screenToCanvas(e.clientX, e.clientY);
		const hit = findNodeAtPoint(canvas.x, canvas.y);
		catalogDragHoverNodeId = hit?.id ?? null;
	}

	function onCanvasDragLeave() {
		catalogDragHoverNodeId = null;
	}

	function onCanvasDrop(e: DragEvent) {
		e.preventDefault();
		e.stopPropagation();
		catalogDragHoverNodeId = null;

		const data = e.dataTransfer?.getData('application/x-workflow-node');
		if (!data) return;

		try {
			const item = JSON.parse(data);
			const canvas = screenToCanvas(e.clientX, e.clientY);
			const hit = findNodeAtPoint(canvas.x, canvas.y);

			if (hit) {
				ondropNode?.(item, hit.id);
			} else {
				// Find the last node in the chain (no outgoing edges)
				const outgoing = new Set<string>();
				for (const edge of displayEdges) outgoing.add(edge.from.id);
				const terminals = displayNodes.filter(n => n.type === 'activity' && !outgoing.has(n.id));
				const afterId = terminals.length > 0 ? terminals[terminals.length - 1].id : null;
				ondropNode?.(item, afterId);
			}
		} catch { /* invalid data */ }
	}

	// ── Coordinate helpers
	function screenToCanvas(screenX: number, screenY: number): { x: number; y: number } {
		if (!container) return { x: 0, y: 0 };
		const rect = container.getBoundingClientRect();
		return {
			x: (screenX - rect.left - pan.x) / zoom,
			y: (screenY - rect.top - pan.y) / zoom,
		};
	}

	function findNodeAtPoint(cx: number, cy: number, excludeId?: string): LayoutWorkflowNode | null {
		for (const node of displayNodes) {
			if (excludeId && node.id === excludeId) continue;
			if (cx >= node.x && cx <= node.x + node.w && cy >= node.y && cy <= node.y + node.h) {
				return node;
			}
		}
		return null;
	}

	// Re-fit when workflow changes
	let prevWfName = '';
	$effect(() => {
		if (workflowName !== prevWfName) {
			prevWfName = workflowName;
			initialized = false;
		}
	});

	// Auto-fit on first load / workflow switch
	$effect(() => {
		if (initialized || displayNodes.length === 0 || !container) return;
		initialized = true;
		const fit = fitToContainer(bounds, container.clientWidth, container.clientHeight);
		zoom = fit.zoom;
		pan = fit.pan;
	});

	function zoomToward(newZoom: number, point: { x: number; y: number }) {
		const clamped = clampZoom(newZoom);
		const scale = clamped / zoom;
		pan = {
			x: point.x - scale * (point.x - pan.x),
			y: point.y - scale * (point.y - pan.y),
		};
		zoom = clamped;
	}

	function doFitToScreen() {
		if (!container) return;
		const fit = fitToContainer(bounds, container.clientWidth, container.clientHeight);
		zoom = fit.zoom;
		pan = fit.pan;
	}

	// ── Event handlers ──

	function onContainerMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		const target = e.target as HTMLElement;

		// Output handle → start wire drag
		const handle = target.closest('[data-wf-handle]');
		if (handle && mode === 'edit') {
			e.preventDefault();
			e.stopPropagation();
			const nodeId = handle.getAttribute('data-wf-handle')!;
			const node = displayNodes.find(n => n.id === nodeId);
			if (node) {
				wireDrag = {
					sourceId: nodeId,
					startX: node.x + node.w,
					startY: node.y + node.h / 2,
					currentX: node.x + node.w + 20,
					currentY: node.y + node.h / 2,
					hoverNodeId: null,
				};
			}
			return;
		}

		// Node → start node drag
		const nodeEl = target.closest('[data-wf-node]');
		if (nodeEl && mode === 'edit') {
			e.stopPropagation();
			const nodeId = nodeEl.getAttribute('data-node-id')!;
			const node = displayNodes.find(n => n.id === nodeId);
			if (node) {
				nodeDrag = {
					id: nodeId,
					startMouseX: e.clientX,
					startMouseY: e.clientY,
					startNodeX: node.x,
					startNodeY: node.y,
				};
				hasDragged = false;
			}
			return;
		}

		// Plus button — ignore (handled by onclick)
		if (target.closest('[data-wf-plus]')) return;

		// SVG edge hit area — ignore (handled by its own onclick)
		if (target.closest('[data-wf-edge]')) return;

		// Canvas background → deselect + start pan
		contextMenu = null;
		selectedEdgeKey = null;
		onselect?.(null);
		panning = true;
		panStart = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
	}

	function onContainerMouseMove(e: MouseEvent) {
		if (wireDrag) {
			const canvas = screenToCanvas(e.clientX, e.clientY);
			const hit = findNodeAtPoint(canvas.x, canvas.y, wireDrag.sourceId);
			wireDrag = { ...wireDrag, currentX: canvas.x, currentY: canvas.y, hoverNodeId: hit?.id ?? null };
			return;
		}

		if (nodeDrag) {
			const dx = (e.clientX - nodeDrag.startMouseX) / zoom;
			const dy = (e.clientY - nodeDrag.startMouseY) / zoom;
			if (!hasDragged && (Math.abs(dx) > 3 || Math.abs(dy) > 3)) hasDragged = true;
			if (hasDragged) {
				posOverrides = {
					...posOverrides,
					[nodeDrag.id]: {
						x: nodeDrag.startNodeX + dx,
						y: nodeDrag.startNodeY + dy,
					},
				};
			}
			return;
		}

		if (panning) {
			pan = {
				x: panStart.panX + (e.clientX - panStart.x),
				y: panStart.panY + (e.clientY - panStart.y),
			};
		}
	}

	function onContainerMouseUp(e: MouseEvent) {
		if (wireDrag) {
			const canvas = screenToCanvas(e.clientX, e.clientY);
			const target = findNodeAtPoint(canvas.x, canvas.y, wireDrag.sourceId);
			if (target) {
				oncreateConnection?.(wireDrag.sourceId, target.id);
			} else {
				onopenCatalog?.(wireDrag.sourceId, '__parallel__');
			}
			wireDrag = null;
			return;
		}

		if (nodeDrag) {
			if (!hasDragged) {
				const node = displayNodes.find(n => n.id === nodeDrag!.id);
				if (node) {
					contextMenu = null;
					selectedEdgeKey = null;
					if (node.type === 'trigger' || node.type === 'emit') {
						onselect?.(null);
					} else {
						onselect?.(node.id);
					}
				}
			}
			nodeDrag = null;
			return;
		}

		panning = false;
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		if (!container) return;
		const rect = container.getBoundingClientRect();
		zoomToward(zoom * (e.deltaY < 0 ? 1.1 : 0.9), { x: e.clientX - rect.left, y: e.clientY - rect.top });
	}

	function handleNodeContextMenu(e: MouseEvent, nodeId: string) {
		if (mode !== 'edit') return;
		e.preventDefault();
		contextMenu = { x: e.clientX, y: e.clientY, nodeId };
	}

	function handleEdgeClick(e: MouseEvent, edge: WorkflowEdge) {
		if (mode !== 'edit') return;
		e.stopPropagation();
		selectedEdgeKey = edgeKey(edge);
		onselect?.(null);
	}

	function handleEdgeContextMenu(e: MouseEvent, edge: WorkflowEdge) {
		if (mode !== 'edit') return;
		e.preventDefault();
		e.stopPropagation();
		selectedEdgeKey = edgeKey(edge);
		contextMenu = { x: e.clientX, y: e.clientY, edgeFrom: edge.from.id, edgeTo: edge.to.id };
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (mode !== 'edit') return;
		if (e.key === 'Delete' || e.key === 'Backspace') {
			if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement) return;
			if (selectedEdgeKey) {
				const parts = selectedEdgeKey.split('→');
				if (parts.length === 2) {
					onremoveConnection?.(parts[0], parts[1]);
					selectedEdgeKey = null;
				}
				return;
			}
			if (selectedNodeId && selectedNodeId !== '__trigger__') {
				onremove?.(selectedNodeId);
			}
		}
		if (e.key === 'Escape') {
			contextMenu = null;
			wireDrag = null;
			selectedEdgeKey = null;
			onselect?.(null);
		}
	}

	function statusBorder(node: LayoutWorkflowNode): string {
		if (node.type === 'trigger') return 'border-primary';
		if (node.type === 'emit') return 'border-accent';
		if (node.status === 'success') return 'border-success';
		if (node.status === 'failed') return 'border-error';
		if (node.status === 'running') return 'border-warning';
		if (node.activityType && node.activityType !== 'custom') {
			return getActivityType(node.activityType).accentClass;
		}
		return 'border-base-300';
	}

	function statusBg(node: LayoutWorkflowNode): string {
		if (node.type === 'trigger') return 'bg-primary/5';
		if (node.type === 'emit') return 'bg-accent/5';
		if (node.status === 'success') return 'bg-success/5';
		if (node.status === 'failed') return 'bg-error/5';
		return 'bg-base-100';
	}

	function cursorStyle(): string {
		if (wireDrag) return 'crosshair';
		if (nodeDrag && hasDragged) return 'grabbing';
		if (panning) return 'grabbing';
		return 'grab';
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	bind:this={container}
	class="relative h-full w-full overflow-hidden outline-none"
	style="cursor: {cursorStyle()}; touch-action: none;"
	onmousedown={onContainerMouseDown}
	onmousemove={onContainerMouseMove}
	onmouseup={onContainerMouseUp}
	onmouseleave={() => { panning = false; nodeDrag = null; wireDrag = null; }}
	onwheel={onWheel}
	onkeydown={handleKeyDown}
	ondragover={onCanvasDragOver}
	ondragleave={onCanvasDragLeave}
	ondrop={onCanvasDrop}
	tabindex="-1"
>
	{#if displayNodes.length === 0}
		<div class="flex h-full items-center justify-center flex-col gap-3">
			<div class="text-3xl text-base-content/20">+</div>
			<span class="text-xs text-base-content/50">No activities yet — add a node to get started</span>
		</div>
	{:else}
		<!-- Dot grid background -->
		<svg class="pointer-events-none absolute inset-0 h-full w-full text-base-content/5">
			<defs>
				<pattern id="wf-builder-grid" width="20" height="20" patternUnits="userSpaceOnUse"
					patternTransform="translate({pan.x}, {pan.y}) scale({zoom})">
					<circle cx="10" cy="10" r="0.8" fill="currentColor" />
				</pattern>
			</defs>
			<rect width="100%" height="100%" fill="url(#wf-builder-grid)" />
		</svg>

		<!-- Zoom controls -->
		<div class="absolute right-3 top-3 z-10 flex flex-col gap-1.5">
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Zoom in"
				onclick={() => container && zoomToward(zoom * 1.2, { x: container.clientWidth / 2, y: container.clientHeight / 2 })}
			>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
			</button>
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Zoom out"
				onclick={() => container && zoomToward(zoom * 0.8, { x: container.clientWidth / 2, y: container.clientHeight / 2 })}
			>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="5" y1="12" x2="19" y2="12"/></svg>
			</button>
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Fit to screen"
				onclick={doFitToScreen}
			>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
			</button>
		</div>

		<!-- SVG layer for edges + wire drag -->
		<svg class="absolute inset-0 h-full w-full" style="z-index: 1;">
			<g transform="translate({pan.x}, {pan.y}) scale({zoom})">
				{#each displayEdges as edge, ei (`edge-${ei}-${edge.from.id}-${edge.to.id}`)}
					{@const key = edgeKey(edge)}
					{@const isEdgeSelected = selectedEdgeKey === key}
					{@const isEdgeHovered = hoveredEdgeKey === key}
					<!-- Invisible wide hit area for clicking edges -->
					<path
						data-wf-edge
						d={edgePath(edge)}
						fill="none"
						stroke="transparent"
						stroke-width="16"
						class="cursor-pointer"
						role="presentation"
						style="pointer-events: stroke;"
						onclick={(e) => handleEdgeClick(e, edge)}
						oncontextmenu={(e) => handleEdgeContextMenu(e, edge)}
						onmouseenter={() => { hoveredEdgeKey = key; }}
						onmouseleave={() => { hoveredEdgeKey = null; }}
					/>
					<!-- Visible edge -->
					<path
						d={edgePath(edge)}
						fill="none"
						stroke="currentColor"
						stroke-width={isEdgeSelected ? 3 : 2}
						class="{isEdgeSelected ? 'text-primary' : isEdgeHovered ? 'text-primary/50' : 'text-base-content/20'}"
						style="pointer-events: none;"
					/>
					{@const x2 = edge.to.x}
					{@const y2 = edge.to.y + edge.to.h / 2}
					<polygon
						points="{x2} {y2}, {x2 - 7} {y2 - 4}, {x2 - 7} {y2 + 4}"
						class="{isEdgeSelected ? 'fill-primary' : isEdgeHovered ? 'fill-primary/50' : 'fill-base-content/20'}"
						style="pointer-events: none;"
					/>
					{#if edge.label}
						{@const lx = edge.from.x + edge.from.w + 12}
						{@const ly = edge.from.y + edge.from.h / 2 + (edge.to.y > edge.from.y + 10 ? 6 : edge.to.y < edge.from.y - 10 ? -12 : -3)}
						<rect
							x={lx - 4}
							y={ly - 9}
							width={edge.label.length * 6.5 + 10}
							height="16"
							rx="4"
							class="fill-base-100"
							style="pointer-events: none;"
						/>
						<text
							x={lx}
							y={ly + 2}
							font-size="10"
							font-weight="500"
							class="{edge.label === 'True' || edge.label === 'Each item' ? 'fill-success' : edge.label === 'False' || edge.label === 'Done' ? 'fill-error/70' : 'fill-base-content/60'}"
							style="pointer-events: none;"
						>{edge.label}</text>
					{/if}
					<!-- Delete badge on selected edge -->
					{#if isEdgeSelected && mode === 'edit'}
						{@const mx = (edge.from.x + edge.from.w + edge.to.x) / 2}
						{@const my = (edge.from.y + edge.from.h / 2 + edge.to.y + edge.to.h / 2) / 2}
						<g
							class="cursor-pointer"
							role="presentation"
							style="pointer-events: all;"
							onclick={(e) => { e.stopPropagation(); onremoveConnection?.(edge.from.id, edge.to.id); selectedEdgeKey = null; }}
						>
							<circle cx={mx} cy={my} r="10" class="fill-error/90" />
							<line x1={mx - 3.5} y1={my - 3.5} x2={mx + 3.5} y2={my + 3.5} stroke="white" stroke-width="2" stroke-linecap="round" />
							<line x1={mx + 3.5} y1={my - 3.5} x2={mx - 3.5} y2={my + 3.5} stroke="white" stroke-width="2" stroke-linecap="round" />
						</g>
					{/if}
				{/each}

				<!-- Temporary wire while dragging from an output handle -->
				{#if wireDrag}
					{@const cpOffset = Math.min(Math.abs(wireDrag.currentX - wireDrag.startX) * 0.5, 60)}
					<path
						d="M {wireDrag.startX} {wireDrag.startY} C {wireDrag.startX + cpOffset} {wireDrag.startY}, {wireDrag.currentX - cpOffset} {wireDrag.currentY}, {wireDrag.currentX} {wireDrag.currentY}"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-dasharray="6 3"
						class="text-primary/60"
						style="pointer-events: none;"
					/>
					<circle
						cx={wireDrag.currentX}
						cy={wireDrag.currentY}
						r="4"
						class="fill-primary/40"
						style="pointer-events: none;"
					/>
				{/if}
			</g>
		</svg>

		<!-- Node layer -->
		<div
			class="absolute inset-0"
			style="transform: translate({pan.x}px, {pan.y}px) scale({zoom}); transform-origin: 0 0; z-index: 2;"
		>
			{#each displayNodes as node, ni (`node-${ni}-${node.id}`)}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					data-wf-node
					data-node-id={node.id}
					class="absolute select-none rounded-lg border-2 shadow-sm transition-[box-shadow,border-color] duration-150
						{statusBorder(node)} {statusBg(node)}
						{nodeDrag?.id === node.id && hasDragged ? 'shadow-xl z-50 opacity-90' : ''}
						{wireDrag?.hoverNodeId === node.id || catalogDragHoverNodeId === node.id ? 'ring-2 ring-primary shadow-lg' : ''}
						{selectedNodeId === node.id ? 'ring-2 ring-primary/30 shadow-lg' : 'hover:shadow-md'}
						{mode === 'edit' ? 'cursor-grab' : 'cursor-pointer'}"
					style="left: {node.x}px; top: {node.y}px; width: {node.w}px; height: {node.h}px;"
					oncontextmenu={(e) => handleNodeContextMenu(e, node.id)}
				>
					{#if node.type === 'trigger'}
						<div class="flex flex-col items-center justify-center h-full px-3">
							<div class="text-xs font-semibold text-primary uppercase tracking-wider">{node.label}</div>
							<div class="text-xs text-base-content/60 font-mono truncate max-w-full mt-0.5">{node.sublabel}</div>
						</div>
					{:else if node.type === 'emit'}
						<div class="flex flex-col items-center justify-center h-full px-3">
							<div class="text-xs font-semibold text-accent uppercase tracking-wider">Emit</div>
							<div class="text-xs text-base-content/60 font-mono truncate max-w-full mt-0.5">{node.sublabel}</div>
						</div>
					{:else}
						{@const typeDef = getActivityType(node.activityType)}
						{@const isBranch = isBranchingType(node.activityType)}
						<div class="flex flex-col justify-between h-full px-3 py-2.5 {isBranch ? 'pr-5' : ''}">
							<div class="flex items-center gap-2 min-w-0">
								<span class="text-sm shrink-0">{typeDef.icon}</span>
								<span class="text-sm font-medium truncate">{node.label}</span>
								{#if node.status && node.status !== 'idle'}
									<span class="ml-auto w-2 h-2 rounded-full shrink-0 {node.status === 'success' ? 'bg-success' : node.status === 'failed' ? 'bg-error' : 'bg-warning animate-pulse'}"></span>
								{/if}
							</div>
							<div class="text-xs text-base-content/60 truncate">{node.sublabel}</div>
							<div class="flex items-center gap-1.5">
								{#if node.activityType && node.activityType !== 'custom'}
									<span class="text-xs text-base-content/40">{typeDef.label}</span>
									<span class="text-base-content/20">&middot;</span>
								{/if}
								{#if isBranch && typeDef.branchLabels}
									<span class="text-xs text-base-content/40 font-mono">{typeDef.branchLabels.join(' / ')}</span>
								{:else}
									<span class="text-xs text-base-content/40 font-mono">{node.stepCount ?? 0} steps</span>
								{/if}
							</div>
						</div>
						{#if isBranch && typeDef.branchLabels}
							<div class="absolute right-0 top-0 bottom-0 flex flex-col justify-center gap-3 pr-1">
								{#each typeDef.branchLabels as bl, bi}
									<div class="w-2.5 h-2.5 rounded-full border-2 {bi === 0 ? 'border-success bg-success/20' : 'border-error/60 bg-error/10'}" title={bl}></div>
								{/each}
							</div>
						{/if}
					{/if}

					<!-- Output handle (drag to connect) -->
					{#if mode === 'edit' && node.type !== 'emit'}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							data-wf-handle={node.id}
							class="absolute -right-2.5 top-1/2 -translate-y-1/2 w-5 h-5 rounded-full border-2 border-base-content/25 bg-base-100 cursor-crosshair flex items-center justify-center hover:border-primary hover:bg-primary/10 hover:scale-125 transition-all z-10"
							title="Drag to connect"
						>
							<div class="w-1.5 h-1.5 rounded-full bg-base-content/30"></div>
						</div>
					{/if}

					<!-- Input indicator (drop target highlight) -->
					{#if mode === 'edit' && node.type !== 'trigger' && wireDrag && wireDrag.hoverNodeId === node.id}
						<div
							class="absolute -left-2 top-1/2 -translate-y-1/2 w-4 h-4 rounded-full bg-primary/30 border-2 border-primary animate-pulse"
						></div>
					{/if}
				</div>
			{/each}

			<!-- "+" connector buttons -->
			{#each plusButtons as btn, bi (`plus-${bi}-${btn.afterNodeId}-${btn.branchLabel ?? ''}`)}
				{#if btn.branchLabel}
					<div
						data-wf-plus
						class="absolute -translate-x-1/2 -translate-y-1/2 flex items-center gap-1 cursor-pointer group"
						style="left: {btn.x}px; top: {btn.y}px;"
						role="button"
						tabindex="0"
						onclick={(e) => { e.stopPropagation(); onopenCatalog?.(btn.afterNodeId, btn.branchLabel); }}
						onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); onopenCatalog?.(btn.afterNodeId, btn.branchLabel); } }}
						title="Add {btn.branchLabel} branch"
					>
						<div class="w-6 h-6 rounded-full border-2 border-dashed border-base-content/20 bg-base-100 flex items-center justify-center text-base-content/40 text-xs group-hover:border-primary group-hover:text-primary group-hover:bg-primary/5 transition-colors">+</div>
						<span class="text-xs text-base-content/40 group-hover:text-primary transition-colors">{btn.branchLabel}</span>
					</div>
				{:else}
					<div
						data-wf-plus
						class="absolute w-7 h-7 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-dashed border-base-content/20 bg-base-100 flex items-center justify-center text-base-content/40 text-sm cursor-pointer hover:border-primary hover:text-primary hover:bg-primary/5 transition-colors"
						style="left: {btn.x}px; top: {btn.y}px;"
						role="button"
						tabindex="0"
						onclick={(e) => { e.stopPropagation(); onopenCatalog?.(btn.afterNodeId); }}
						onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); onopenCatalog?.(btn.afterNodeId); } }}
						title="Add node"
					>+</div>
				{/if}
			{/each}
		</div>
	{/if}

	<!-- Context menu -->
	{#if contextMenu}
		<div class="fixed inset-0 z-[80]" role="presentation" onclick={() => contextMenu = null} onmousedown={(e) => e.stopPropagation()}>
			<div
				class="absolute bg-base-100 border border-base-300 rounded-lg shadow-xl py-1 min-w-[160px] z-[81]"
				style="left: {contextMenu.x}px; top: {contextMenu.y}px;"
				role="presentation"
				onclick={(e) => e.stopPropagation()}
			>
				{#if contextMenu.edgeFrom && contextMenu.edgeTo}
					<!-- Edge context menu -->
					<button
						class="w-full text-left px-3 py-1.5 text-xs hover:bg-error/10 text-error cursor-pointer bg-transparent border-none flex items-center gap-2"
						onclick={() => { if (contextMenu?.edgeFrom && contextMenu?.edgeTo) { onremoveConnection?.(contextMenu.edgeFrom, contextMenu.edgeTo); selectedEdgeKey = null; contextMenu = null; } }}
					>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
						Delete Connection
					</button>
				{:else if contextMenu.nodeId && contextMenu.nodeId !== '__trigger__' && contextMenu.nodeId !== '__emit__'}
					<!-- Node context menu -->
					<button
						class="w-full text-left px-3 py-1.5 text-xs hover:bg-base-200 cursor-pointer bg-transparent border-none flex items-center gap-2"
						onclick={() => { if (contextMenu?.nodeId) { onopenCatalog?.(contextMenu.nodeId, '__parallel__'); contextMenu = null; } }}
					>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="18" cy="18" r="3"/><path d="M6 9a9 9 0 0 0 9 9"/><path d="M6 3a9 9 0 0 1 9 3"/></svg>
						Add Parallel Path
					</button>
					<button
						class="w-full text-left px-3 py-1.5 text-xs hover:bg-base-200 cursor-pointer bg-transparent border-none flex items-center gap-2"
						onclick={() => { if (contextMenu?.nodeId) { onduplicate?.(contextMenu.nodeId); contextMenu = null; } }}
					>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
						Duplicate Node
					</button>
					<button
						class="w-full text-left px-3 py-1.5 text-xs hover:bg-error/10 text-error cursor-pointer bg-transparent border-none flex items-center gap-2"
						onclick={() => { if (contextMenu?.nodeId) { onremove?.(contextMenu.nodeId); contextMenu = null; } }}
					>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
						Delete Node
					</button>
				{:else if contextMenu.nodeId === '__emit__'}
					<button
						class="w-full text-left px-3 py-1.5 text-xs hover:bg-error/10 text-error cursor-pointer bg-transparent border-none flex items-center gap-2"
						onclick={() => { if (contextMenu?.nodeId) { onremove?.(contextMenu.nodeId); contextMenu = null; } }}
					>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
						Remove Emit
					</button>
				{/if}
			</div>
		</div>
	{/if}
</div>
