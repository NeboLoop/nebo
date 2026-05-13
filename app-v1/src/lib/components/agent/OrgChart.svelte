<!--
  OrgChart Component
  
  Interactive pannable/zoomable org chart visualization.
  Ported from Paperclip (MIT) — adapted for Svelte 5 + DaisyUI/Tailwind.

  Usage:
    <OrgChart nodes={orgTree} onNodeClick={(node) => goto(`/agents/${node.id}`)} />
-->

<script lang="ts">
	import { Network, Plus, Minus, Maximize2 } from 'lucide-svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import {
		type OrgNode,
		type LayoutNode,
		type Point,
		CARD_W,
		CARD_H,
		TOUCH_MOVE_THRESHOLD,
		layoutForest,
		flattenLayout,
		collectEdges,
		computeBounds,
		clampZoom,
		fitToContainer,
		edgePath
	} from '$lib/utils/orgChartLayout';

	// ── Props ───────────────────────────────────────────────────────────

	let {
		nodes = [],
		loading = false,
		onNodeClick
	}: {
		nodes: OrgNode[];
		loading?: boolean;
		onNodeClick?: (node: LayoutNode) => void;
	} = $props();

	// ── Layout computation (reactive) ───────────────────────────────────

	const layout = $derived(layoutForest(nodes));
	const allNodes = $derived(flattenLayout(layout));
	const edges = $derived(collectEdges(layout));
	const bounds = $derived(computeBounds(allNodes));

	// ── Pan & zoom state ────────────────────────────────────────────────

	let pan = $state<Point>({ x: 0, y: 0 });
	let zoom = $state(1);
	let dragging = $state(false);
	let container: HTMLDivElement | undefined = $state();
	let initialized = false;

	// Drag tracking (not reactive — just mutable refs)
	let dragStart = { x: 0, y: 0, panX: 0, panY: 0 };
	let suppressClick = false;
	let suppressTimer: ReturnType<typeof setTimeout> | null = null;

	// Touch gesture tracking
	let touchMode: 'pan' | 'pinch' | null = null;
	let touchStartPoint: Point = { x: 0, y: 0 };
	let touchStartPan: Point = { x: 0, y: 0 };
	let touchStartZoom = 1;
	let touchStartDistance = 0;
	let touchStartCenter: Point = { x: 0, y: 0 };
	let touchMoved = false;

	// ── Status dot colors ───────────────────────────────────────────────

	const statusColors: Record<string, string> = {
		running: '#22d3ee',
		active: '#4ade80',
		paused: '#facc15',
		idle: '#facc15',
		error: '#f87171',
		terminated: '#a3a3a3'
	};
	const defaultDotColor = '#a3a3a3';

	// ── Auto-fit on first load ──────────────────────────────────────────

	$effect(() => {
		if (initialized || allNodes.length === 0 || !container) return;
		initialized = true;

		const fit = fitToContainer(bounds, container.clientWidth, container.clientHeight);
		zoom = fit.zoom;
		pan = fit.pan;
	});

	// ── Zoom toward a point ─────────────────────────────────────────────

	function zoomToward(newZoom: number, point: Point) {
		const clamped = clampZoom(newZoom);
		const scale = clamped / zoom;
		pan = {
			x: point.x - scale * (point.x - pan.x),
			y: point.y - scale * (point.y - pan.y)
		};
		zoom = clamped;
	}

	function fitToScreen() {
		if (!container) return;
		const fit = fitToContainer(bounds, container.clientWidth, container.clientHeight);
		zoom = fit.zoom;
		pan = fit.pan;
	}

	// ── Mouse handlers ──────────────────────────────────────────────────

	function onMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		const target = e.target as HTMLElement;
		if (target.closest('[data-org-card]')) return;
		dragging = true;
		dragStart = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
	}

	function onMouseMove(e: MouseEvent) {
		if (!dragging) return;
		pan = {
			x: dragStart.panX + (e.clientX - dragStart.x),
			y: dragStart.panY + (e.clientY - dragStart.y)
		};
	}

	function onMouseUp() {
		dragging = false;
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		if (!container) return;

		const rect = container.getBoundingClientRect();
		const mouseX = e.clientX - rect.left;
		const mouseY = e.clientY - rect.top;
		const factor = e.deltaY < 0 ? 1.1 : 0.9;
		zoomToward(zoom * factor, { x: mouseX, y: mouseY });
	}

	// ── Touch helpers ───────────────────────────────────────────────────

	function getTouchDistance(a: Touch, b: Touch): number {
		return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
	}

	function getTouchCenter(a: Touch, b: Touch): Point {
		if (!container) return { x: 0, y: 0 };
		const rect = container.getBoundingClientRect();
		return {
			x: (a.clientX + b.clientX) / 2 - rect.left,
			y: (a.clientY + b.clientY) / 2 - rect.top
		};
	}

	// ── Touch handlers ──────────────────────────────────────────────────

	function onTouchStart(e: TouchEvent) {
		if (e.touches.length >= 2 && container) {
			const [a, b] = [e.touches[0]!, e.touches[1]!];
			touchMode = 'pinch';
			touchStartPan = { ...pan };
			touchStartZoom = zoom;
			touchStartDistance = getTouchDistance(a, b);
			touchStartCenter = getTouchCenter(a, b);
			touchMoved = false;
			return;
		}

		const touch = e.touches[0];
		if (!touch) return;
		touchMode = 'pan';
		touchStartPoint = { x: touch.clientX, y: touch.clientY };
		touchStartPan = { ...pan };
		touchMoved = false;
	}

	function onTouchMove(e: TouchEvent) {
		if (!container || !touchMode) return;

		if (e.touches.length >= 2) {
			const [a, b] = [e.touches[0]!, e.touches[1]!];
			const distance = getTouchDistance(a, b);
			const center = getTouchCenter(a, b);

			if (touchMode !== 'pinch' || touchStartDistance === 0) {
				touchMode = 'pinch';
				touchStartPan = { ...pan };
				touchStartZoom = zoom;
				touchStartDistance = distance;
				touchStartCenter = center;
				touchMoved = false;
				return;
			}

			const nextZoom = clampZoom(touchStartZoom * (distance / touchStartDistance));
			const scale = nextZoom / touchStartZoom;
			const dx = center.x - touchStartCenter.x;
			const dy = center.y - touchStartCenter.y;
			touchMoved =
				touchMoved ||
				Math.abs(distance - touchStartDistance) > TOUCH_MOVE_THRESHOLD ||
				Math.hypot(dx, dy) > TOUCH_MOVE_THRESHOLD;
			zoom = nextZoom;
			pan = {
				x: center.x - scale * (touchStartCenter.x - touchStartPan.x),
				y: center.y - scale * (touchStartCenter.y - touchStartPan.y)
			};
			return;
		}

		const touch = e.touches[0];
		if (!touch || touchMode !== 'pan') return;
		const dx = touch.clientX - touchStartPoint.x;
		const dy = touch.clientY - touchStartPoint.y;
		touchMoved = touchMoved || Math.hypot(dx, dy) > TOUCH_MOVE_THRESHOLD;
		pan = {
			x: touchStartPan.x + dx,
			y: touchStartPan.y + dy
		};
	}

	function onTouchEnd() {
		if (touchMoved) {
			suppressClick = true;
			if (suppressTimer !== null) clearTimeout(suppressTimer);
			suppressTimer = setTimeout(() => {
				suppressClick = false;
				suppressTimer = null;
			}, 400);
		}
		touchMode = null;
		touchStartPan = { ...pan };
		touchStartZoom = zoom;
		touchMoved = false;
	}

	// ── Card click ──────────────────────────────────────────────────────

	function handleCardClick(node: LayoutNode) {
		if (suppressClick) {
			suppressClick = false;
			return;
		}
		onNodeClick?.(node);
	}
</script>

{#if loading}
	<div class="flex h-full items-center justify-center">
		<span class="loading loading-spinner loading-lg"></span>
	</div>
{:else if nodes.length === 0}
	<EmptyState icon={Network} title="No org chart data" message="No organizational hierarchy defined." />
{:else}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		bind:this={container}
		class="relative h-full w-full min-h-[420px] overflow-hidden rounded-lg border border-base-300 bg-base-200/30"
		style="cursor: {dragging ? 'grabbing' : 'grab'}; touch-action: none; overscroll-behavior: contain;"
		onmousedown={onMouseDown}
		onmousemove={onMouseMove}
		onmouseup={onMouseUp}
		onmouseleave={onMouseUp}
		onwheel={onWheel}
		ontouchstart={onTouchStart}
		ontouchmove={onTouchMove}
		ontouchend={onTouchEnd}
		ontouchcancel={onTouchEnd}
	>
		<!-- Zoom controls -->
		<div class="absolute right-3 top-3 z-10 flex flex-col gap-1.5">
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Zoom in"
				onclick={() => {
					if (container) {
						zoomToward(zoom * 1.2, {
							x: container.clientWidth / 2,
							y: container.clientHeight / 2
						});
					}
				}}
			>
				<Plus class="h-3.5 w-3.5" />
			</button>
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Zoom out"
				onclick={() => {
					if (container) {
						zoomToward(zoom * 0.8, {
							x: container.clientWidth / 2,
							y: container.clientHeight / 2
						});
					}
				}}
			>
				<Minus class="h-3.5 w-3.5" />
			</button>
			<button
				class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
				title="Fit to screen"
				onclick={fitToScreen}
			>
				<Maximize2 class="h-3.5 w-3.5" />
			</button>
		</div>

		<!-- SVG layer for edge connectors -->
		<svg class="pointer-events-none absolute inset-0 h-full w-full">
			<g transform="translate({pan.x}, {pan.y}) scale({zoom})">
				{#each edges as edge (`${edge.parent.id}-${edge.child.id}`)}
					<path
						d={edgePath(edge)}
						fill="none"
						stroke="oklch(var(--bc) / 0.2)"
						stroke-width="1.5"
					/>
				{/each}
			</g>
		</svg>

		<!-- Card layer -->
		<div
			class="absolute inset-0"
			style="transform: translate({pan.x}px, {pan.y}px) scale({zoom}); transform-origin: 0 0;"
		>
			{#each allNodes as node (node.id)}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					data-org-card
					class="absolute cursor-pointer select-none rounded-lg border border-base-300 bg-base-100 shadow-sm transition-[box-shadow,border-color] duration-150 hover:border-base-content/20 hover:shadow-md"
					style="left: {node.x}px; top: {node.y}px; width: {CARD_W}px; min-height: {CARD_H}px;"
					onclick={() => handleCardClick(node)}
				>
					<div class="flex items-center gap-3 px-4 py-3">
						<!-- Avatar + status dot -->
						<div class="relative shrink-0">
							<div class="flex h-9 w-9 items-center justify-center rounded-full bg-base-300">
								<span class="text-sm font-bold text-base-content/70">
									{node.name.charAt(0).toUpperCase()}
								</span>
							</div>
							<span
								class="absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border-2 border-base-100"
								style="background-color: {statusColors[node.status] ?? defaultDotColor};"
							></span>
						</div>
						<!-- Name + role -->
						<div class="flex min-w-0 flex-1 flex-col items-start">
							<span class="text-sm font-semibold leading-tight text-base-content">
								{node.name}
							</span>
							<span class="mt-0.5 text-[11px] leading-tight text-base-content/60">
								{node.role}
							</span>
							{#if node.subtitle}
								<span class="mt-1 line-clamp-2 text-[10px] leading-tight text-base-content/50">
									{node.subtitle}
								</span>
							{/if}
						</div>
					</div>
				</div>
			{/each}
		</div>
	</div>
{/if}
