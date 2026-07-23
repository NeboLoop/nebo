<script lang="ts">
	import { t } from 'svelte-i18n';
	import {
		layoutWorkflow,
		clampZoom,
		computeBounds,
		fitToContainer,
		edgePath,
		type LayoutWorkflowNode,
	} from '$lib/utils/workflowLayout';
	import { getActivityType, isBranchingType } from '$lib/utils/workflowTypes';
	import type { WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';

	let {
		workflows = {},
		agentId = '',
	}: {
		workflows: Record<string, WorkflowConfig>;
		agentId: string;
	} = $props();

	// ── Layout computation
	const allLayouts = $derived(() => {
		const entries = Object.entries(workflows);
		const layouts: Array<{
			name: string;
			wf: WorkflowConfig;
			nodes: LayoutWorkflowNode[];
			edges: Array<{ from: LayoutWorkflowNode; to: LayoutWorkflowNode; label?: string }>;
			offsetY: number;
		}> = [];

		let y = 0;
		for (const [name, wf] of entries) {
			const { nodes, edges } = layoutWorkflow(
				wf.trigger || { type: 'manual' },
				wf.activities || [],
				wf.emit,
				undefined,
				wf.connections,
			);

			const shifted = nodes.map(n => ({ ...n, y: n.y + y }));
			const shiftedEdges = edges.map(e => ({
				from: shifted.find(n => n.id === e.from.id)!,
				to: shifted.find(n => n.id === e.to.id)!,
				label: e.label,
			}));

			layouts.push({ name, wf, nodes: shifted, edges: shiftedEdges, offsetY: y });
			const maxY = Math.max(...shifted.map(n => n.y + n.h));
			y = maxY + 60;
		}

		return layouts;
	});

	const allNodes = $derived(allLayouts().flatMap(l => l.nodes));
	const allEdges = $derived(allLayouts().flatMap(l => l.edges));
	const bounds = $derived(computeBounds(allNodes));

	// ── Pan & zoom
	let pan = $state({ x: 0, y: 0 });
	let zoom = $state(1);
	let dragging = $state(false);
	let container = $state<HTMLDivElement | undefined>(undefined);
	let initialized = false;

	let dragStart = { x: 0, y: 0, panX: 0, panY: 0 };

	// ── Selected node + detail panel
	let selectedNodeId = $state<string | null>(null);
	let selectedWorkflowName = $state<string | null>(null);

	const selectedLayout = $derived(selectedWorkflowName ? allLayouts().find(l => l.name === selectedWorkflowName) : null);
	const selectedActivity = $derived.by(() => {
		if (!selectedLayout || !selectedNodeId) return null;
		const wf = selectedLayout.wf;
		return wf.activities?.find((a: WorkflowActivity) => a.id === selectedNodeId) ?? null;
	});
	const selectedWf = $derived(selectedLayout?.wf);
	let expandedSteps = $state<Record<string, boolean>>({});

	// Auto-fit on first load
	$effect(() => {
		if (initialized || allNodes.length === 0 || !container) return;
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

	function onMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		if ((e.target as HTMLElement).closest('[data-wf-node]')) return;
		if ((e.target as HTMLElement).closest('[data-wf-panel]')) return;
		dragging = true;
		dragStart = { x: e.clientX, y: e.clientY, panX: pan.x, panY: pan.y };
	}

	function onMouseMove(e: MouseEvent) {
		if (!dragging) return;
		pan = {
			x: dragStart.panX + (e.clientX - dragStart.x),
			y: dragStart.panY + (e.clientY - dragStart.y),
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

	function handleNodeClick(node: LayoutWorkflowNode, workflowName: string) {
		if (node.type === 'trigger' || node.type === 'emit') {
			// Select the workflow itself
			selectedNodeId = null;
			selectedWorkflowName = workflowName;
		} else {
			selectedNodeId = node.id;
			selectedWorkflowName = workflowName;
		}
		expandedSteps = {};
	}

	function handleCanvasClick() {
		// Deselect when clicking empty canvas
		selectedNodeId = null;
		selectedWorkflowName = null;
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

	function isSelected(nodeId: string, workflowName: string): boolean {
		return selectedWorkflowName === workflowName && (selectedNodeId === nodeId || (selectedNodeId === null && (nodeId === '__trigger__' || nodeId === '__emit__')));
	}
</script>

{#if allNodes.length === 0}
	<div class="flex h-full items-center justify-center">
		<span class="text-xs text-base-content/50">{$t('workflowCanvas.noWorkflows')}</span>
	</div>
{:else}
	<div class="flex h-full w-full">
		<!-- Canvas area -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			bind:this={container}
			class="relative flex-1 min-w-0 overflow-hidden"
			style="cursor: {dragging ? 'grabbing' : 'grab'}; touch-action: none;"
			onmousedown={(e) => {
				if (!(e.target as HTMLElement).closest('[data-wf-node]')) {
					handleCanvasClick();
				}
				onMouseDown(e);
			}}
			onmousemove={onMouseMove}
			onmouseup={onMouseUp}
			onmouseleave={onMouseUp}
			onwheel={onWheel}
		>
			<!-- Dot grid background -->
			<svg class="pointer-events-none absolute inset-0 h-full w-full text-base-content/5">
				<defs>
					<pattern id="wf-grid" width="20" height="20" patternUnits="userSpaceOnUse"
						patternTransform="translate({pan.x}, {pan.y}) scale({zoom})">
						<circle cx="10" cy="10" r="0.8" fill="currentColor" />
					</pattern>
				</defs>
				<rect width="100%" height="100%" fill="url(#wf-grid)" />
			</svg>

			<!-- Zoom controls -->
			<div class="absolute right-3 top-3 z-10 flex flex-col gap-1.5">
				<button
					class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
					title={$t('commander.zoomIn')}
					onclick={() => container && zoomToward(zoom * 1.2, { x: container.clientWidth / 2, y: container.clientHeight / 2 })}
				>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
				</button>
				<button
					class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
					title={$t('commander.zoomOut')}
					onclick={() => container && zoomToward(zoom * 0.8, { x: container.clientWidth / 2, y: container.clientHeight / 2 })}
				>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="5" y1="12" x2="19" y2="12"/></svg>
				</button>
				<button
					class="btn btn-square btn-sm btn-ghost border border-base-300 bg-base-100"
					title={$t('workflowCanvas.fitToScreen')}
					onclick={doFitToScreen}
				>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
				</button>
			</div>

			<!-- Workflow labels -->
			<div
				class="absolute inset-0 pointer-events-none"
				style="transform: translate({pan.x}px, {pan.y}px) scale({zoom}); transform-origin: 0 0;"
			>
				{#each allLayouts() as layout}
					<div
						class="absolute text-xs font-semibold uppercase tracking-wider text-base-content/40"
						style="left: {40}px; top: {layout.offsetY + 12}px;"
					>{layout.name}</div>
				{/each}
			</div>

			<!-- SVG layer for edge connectors -->
			<svg class="pointer-events-none absolute inset-0 h-full w-full text-base-content/20">
				<g transform="translate({pan.x}, {pan.y}) scale({zoom})">
					{#each allEdges as edge, ei (`${ei}-${edge.from.id}-${edge.to.id}`)}
						<path
							d={edgePath(edge)}
							fill="none"
							stroke="currentColor"
							stroke-width="2"
						/>
						<!-- Arrow at end -->
						{@const x2 = edge.to.x}
						{@const y2 = edge.to.y + edge.to.h / 2}
						<polygon
							points="{x2} {y2}, {x2 - 7} {y2 - 4}, {x2 - 7} {y2 + 4}"
							fill="currentColor"
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
							/>
							<text
								x={lx}
								y={ly + 2}
								font-size="10"
								font-weight="500"
								class="{edge.label === 'True' || edge.label === 'Each item' ? 'fill-success' : edge.label === 'False' || edge.label === 'Done' ? 'fill-error/70' : 'fill-base-content/60'}"
							>{edge.label}</text>
						{/if}
					{/each}
				</g>
			</svg>

			<!-- Node layer -->
			<div
				class="absolute inset-0"
				style="transform: translate({pan.x}px, {pan.y}px) scale({zoom}); transform-origin: 0 0;"
			>
				{#each allLayouts() as layout}
					{#each layout.nodes as node, ni (`${layout.name}-${ni}-${node.id}`)}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							data-wf-node
							class="absolute select-none rounded-lg border-2 shadow-sm transition-[box-shadow,border-color] duration-150 cursor-pointer
								{statusBorder(node)} {statusBg(node)}
								{isSelected(node.id, layout.name) ? 'ring-2 ring-primary/30 shadow-lg' : 'hover:shadow-md'}"
							style="left: {node.x}px; top: {node.y}px; width: {node.w}px; height: {node.h}px;"
							onclick={(e) => { e.stopPropagation(); handleNodeClick(node, layout.name); }}
						>
							{#if node.type === 'trigger'}
								<div class="flex flex-col items-center justify-center h-full px-3">
									<div class="text-xs font-semibold text-primary uppercase tracking-wider">{node.label}</div>
									<div class="text-xs text-base-content/60 font-mono truncate max-w-full mt-0.5">{node.sublabel}</div>
								</div>
							{:else if node.type === 'emit'}
								<div class="flex flex-col items-center justify-center h-full px-3">
									<div class="text-xs font-semibold text-accent uppercase tracking-wider">{$t('workflowCanvas.emit')}</div>
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
											<span class="text-xs text-base-content/40 font-mono">{$t('automations.stepCount', { values: { count: node.stepCount ?? 0 } })}</span>
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
						</div>
					{/each}
				{/each}
			</div>
		</div>

		<!-- Detail panel (slides in from right) -->
		{#if selectedWorkflowName && selectedWf}
			<div data-wf-panel class="w-[320px] shrink-0 border-l border-base-content/10 bg-base-100 flex flex-col overflow-hidden">
				<!-- Panel header -->
				<div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10 shrink-0">
					<div class="flex-1 min-w-0">
						<div class="text-sm font-semibold truncate">{selectedWorkflowName}</div>
						<div class="text-xs text-base-content/50">{$t('workflowCanvas.activityCount', { values: { count: selectedWf.activities?.length ?? 0 } })}</div>
					</div>
					<div class="flex items-center gap-1.5 shrink-0">
						<input type="checkbox" class="toggle toggle-sm toggle-primary" checked={selectedWf.isActive !== false} role="switch" title={$t('workflowCanvas.enableDisable')} />
						<button class="w-6 h-6 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base" onclick={() => { selectedNodeId = null; selectedWorkflowName = null; }}>&times;</button>
					</div>
				</div>

				<div class="flex-1 overflow-y-auto p-4">
					{#if selectedNodeId && selectedActivity}
						<!-- Activity detail -->
						{@const act = selectedActivity}
						<div class="mb-4">
							<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.activity')}</div>
							<div class="text-sm font-medium">{act.id}</div>
							<div class="text-xs text-base-content/70 mt-0.5">{act.intent}</div>
						</div>

						{#if act.skills?.length}
							<div class="mb-4">
								<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('marketplace.skills')}</div>
								<div class="flex flex-col gap-1">
									{#each act.skills as skill}
										<div class="py-1 px-2 rounded bg-base-200 font-mono text-xs truncate">{skill}</div>
									{/each}
								</div>
							</div>
						{/if}

						{#if act.steps?.length}
							<div>
								<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('automations.steps')}</div>
								<div class="flex flex-col gap-1">
									{#each act.steps as step, i}
										<div class="flex items-start gap-2 py-1.5 px-2 rounded-md border border-base-300 bg-base-100">
											<span class="font-mono text-xs text-base-content/40 shrink-0 mt-px w-3 text-right">{i + 1}</span>
											<span class="text-xs flex-1">{step}</span>
										</div>
									{/each}
								</div>
							</div>
						{/if}
					{:else}
						<!-- Workflow overview -->
						<div class="mb-4">
							<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('automations.trigger')}</div>
							<div class="flex items-center gap-1.5">
								<span class="text-sm capitalize">{selectedWf.trigger?.type ?? 'manual'}</span>
								{#if selectedWf.trigger?.schedule}
									<span class="text-xs text-base-content/50 font-mono">&middot; {selectedWf.trigger.schedule}</span>
								{/if}
								{#if selectedWf.trigger?.event}
									<span class="text-xs text-base-content/50 font-mono">&middot; {selectedWf.trigger.event}</span>
								{/if}
							</div>
						</div>

						<div class="mb-4">
							<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('agentSettings.descriptionLabel')}</div>
							<div class="text-xs text-base-content/70 leading-relaxed">{selectedWf.description}</div>
						</div>

						{#if selectedWf.emit}
							<div class="mb-4">
								<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.emits')}</div>
								<div class="py-1 px-2 rounded bg-accent/10 text-xs text-accent font-mono inline-block">{selectedWf.emit}</div>
							</div>
						{/if}

						{#if selectedWf.lastFired}
							<div class="mb-4">
								<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('workflowCanvas.lastFired')}</div>
								<div class="text-xs text-base-content/70 font-mono">{selectedWf.lastFired}</div>
							</div>
						{/if}

						<!-- Activity list -->
						<div>
							<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('workflowCanvas.activities')}</div>
							<div class="flex flex-col gap-1.5">
								{#each selectedWf.activities ?? [] as act, idx}
									<button
										class="w-full flex items-start gap-2.5 p-2.5 rounded-lg border text-left cursor-pointer transition-colors bg-transparent
											{selectedNodeId === act.id ? 'border-primary bg-primary/5' : 'border-base-300 hover:border-base-content/20'}"
										onclick={() => { selectedNodeId = act.id; }}
									>
										<div class="w-5 h-5 rounded-full bg-base-200 flex items-center justify-center font-mono text-xs font-semibold shrink-0">{idx + 1}</div>
										<div class="flex-1 min-w-0">
											<div class="text-sm font-medium truncate">{act.id}</div>
											<div class="text-xs text-base-content/60 truncate">{act.intent}</div>
											<div class="text-xs text-base-content/40 font-mono mt-0.5">{$t('automations.stepCount', { values: { count: act.steps?.length ?? 0 } })}</div>
										</div>
									</button>
								{/each}
							</div>
						</div>
					{/if}
				</div>

				<!-- Panel footer -->
				{#if selectedNodeId}
					<div class="px-4 py-3 border-t border-base-content/10 shrink-0">
						<button
							class="text-xs text-primary cursor-pointer bg-transparent border-none hover:underline p-0"
							onclick={() => selectedNodeId = null}
						>{$t('workflowCanvas.backToOverview')}</button>
					</div>
				{/if}
			</div>
		{/if}
	</div>
{/if}
