<script lang="ts">
	import { onMount } from 'svelte';
	import {
		SvelteFlow,
		Background,
		type Node,
		type Edge,
		type NodeTypes,
		type EdgeTypes,
		type Connection,
	} from '@xyflow/svelte';
	import dagre from '@dagrejs/dagre';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { webapi } from '$lib/api/gocliRequest';

	import AgentNode from './AgentNode.svelte';
	import MainBotNode from './MainBotNode.svelte';
	import TeamGroup from './TeamGroup.svelte';
	import EventEdge from './EventEdge.svelte';
	import CommanderToolbar from './CommanderToolbar.svelte';
	import AgentNodeDetail from './AgentNodeDetail.svelte';

	const nodeTypes: NodeTypes = {
		agent: AgentNode as any,
		main: MainBotNode as any,
		team: TeamGroup as any,
	};

	// Use SvelteFlow's built-in edge types — custom edges break click handling
	const edgeTypes: EdgeTypes = {};

	let nodes = $state.raw<Node[]>([]);
	let edges = $state.raw<Edge[]>([]);
	let selectedNode = $state<Node | null>(null);
	let detailOpen = $state(false);
	let loading = $state(true);
	let selectedEdgeId = $state<string | null>(null);

	let saveTimeout: ReturnType<typeof setTimeout> | null = null;

	async function loadGraph() {
		try {
			const data: any = await webapi.get('/api/v1/commander/graph');
			const graphNodes: Node[] = [];
			const graphEdges: Edge[] = [];

			for (const n of data.nodes ?? []) {
				graphNodes.push({
					id: n.id,
					type: n.type === 'main' ? 'main' : 'agent',
					position: n.position ?? { x: 0, y: 0 },
					data: {
						name: n.name,
						description: n.description,
						status: n.status ?? 'active',
						workflowCount: n.workflowCount ?? 0,
					},
					parentId: undefined,
				});
			}

			// Team group nodes
			for (const t of data.teams ?? []) {
				graphNodes.push({
					id: `team-${t.id}`,
					type: 'team',
					position: t.position ?? { x: 0, y: 0 },
					data: { name: t.name, color: t.color },
					style: 'width: 300px; height: 200px;',
				});
				for (const memberId of t.memberIds ?? []) {
					const member = graphNodes.find((n) => n.id === memberId);
					if (member) {
						member.parentId = `team-${t.id}`;
					}
				}
			}

			// Edges (both computed event edges and user-drawn edges)
			for (const e of data.edges ?? []) {
				graphEdges.push({
					id: e.id,
					source: e.source,
					target: e.target,
					type: 'default',
					label: e.type === 'event' ? e.label : '',
					animated: e.type === 'event',
					selectable: true,
					style: e.type === 'can_chat'
						? 'stroke-dasharray: 5 5; stroke: #7c3aed;'
						: e.type === 'reports_to'
							? 'stroke: #6b7280; stroke-width: 2;'
							: undefined,
				});
			}

			// Auto-layout: use saved positions if available, otherwise dagre
			const hasPositions = data.nodes?.some((n: any) => n.position);
			if (!hasPositions) {
				applyDagreLayout(graphNodes, graphEdges);
			}

			nodes = graphNodes;
			edges = graphEdges;
		} catch (err) {
			console.error('Failed to load commander graph:', err);
		} finally {
			loading = false;
		}
	}

	function applyDagreLayout(layoutNodes: Node[], layoutEdges: Edge[]) {
		const g = new dagre.graphlib.Graph();
		g.setDefaultEdgeLabel(() => ({}));
		g.setGraph({ rankdir: 'TB', nodesep: 100, ranksep: 100, marginx: 40, marginy: 40 });

		const nodeWidth = 220;
		const nodeHeight = 65;
		const mainNodeWidth = 260;
		const mainNodeHeight = 70;

		for (const node of layoutNodes) {
			if (node.type === 'team') continue;
			const w = node.type === 'main' ? mainNodeWidth : nodeWidth;
			const h = node.type === 'main' ? mainNodeHeight : nodeHeight;
			g.setNode(node.id, { width: w, height: h });
		}

		// Only use reporting edges for layout (not event edges)
		for (const edge of layoutEdges) {
			if (!edge.id.startsWith('emit-')) {
				g.setEdge(edge.source, edge.target);
			}
		}

		// Add unconnected nodes to main-bot so they appear in the tree
		const connectedNodes = new Set<string>();
		for (const edge of layoutEdges) {
			if (!edge.id.startsWith('emit-')) {
				connectedNodes.add(edge.source);
				connectedNodes.add(edge.target);
			}
		}
		const mainBot = layoutNodes.find((n) => n.id === 'main-bot');
		if (mainBot) {
			for (const node of layoutNodes) {
				if (node.type === 'team' || node.id === 'main-bot') continue;
				if (!connectedNodes.has(node.id)) {
					g.setEdge('main-bot', node.id);
				}
			}
		}

		dagre.layout(g);

		for (const node of layoutNodes) {
			if (node.type === 'team') continue;
			const dagreNode = g.node(node.id);
			if (dagreNode) {
				node.position = {
					x: dagreNode.x - (node.type === 'main' ? mainNodeWidth / 2 : nodeWidth / 2),
					y: dagreNode.y - (node.type === 'main' ? mainNodeHeight / 2 : nodeHeight / 2),
				};
			}
		}
	}

	function handleAutoLayout() {
		const layoutNodes = [...nodes];
		const layoutEdges = [...edges];
		applyDagreLayout(layoutNodes, layoutEdges);
		nodes = layoutNodes;
		savePositions();
	}

	function handleNodeClick({ node }: { node: Node; event: MouseEvent }) {
		if (node.type === 'main' || node.type === 'team') return;
		selectedEdgeId = null;
		edges = edges.map((e) => ({ ...e, selected: false }));
		selectedNode = node;
		detailOpen = true;
	}

	function handleNodeDragStop({ targetNode }: { targetNode: Node; event: MouseEvent; nodes: Node[] }) {
		nodes = nodes.map((n) =>
			n.id === targetNode.id ? { ...n, position: targetNode.position } : n
		);
		scheduleSavePositions();
	}

	// Handle new connections drawn between nodes — creates a reporting edge
	async function handleConnect(connection: Connection) {
		if (!connection.source || !connection.target) return;
		// Determine edge type: a node can only have ONE "reports_to" inbound edge.
		// If the target already has a reports_to edge, this new connection is "can_chat".
		const targetHasManager = edges.some(
			(e) => e.target === connection.target && (e.data?.edgeType === 'reports_to' || e.type === 'reports_to')
		);
		const edgeType = targetHasManager ? 'can_chat' : 'reports_to';

		try {
			const result: any = await webapi.post('/api/v1/commander/edges', {
				source: connection.source,
				target: connection.target,
				edgeType: edgeType,
				label: '',
			});
			if (result?.edge) {
				edges = [
					...edges,
					{
						id: result.edge.id,
						source: result.edge.sourceNodeId,
						target: result.edge.targetNodeId,
						type: 'default',
						animated: false,
						selectable: true,
						style: edgeType === 'can_chat'
							? 'stroke-dasharray: 5 5; stroke: #7c3aed;'
							: 'stroke: #6b7280; stroke-width: 2;',
					},
				];
			}
		} catch (err) {
			console.error('Failed to create edge:', err);
		}
	}

	// Track which edge is selected via click
	function handleEdgeClick({ event, edge }: { event: MouseEvent; edge: Edge }) {
		console.log('EDGE CLICKED:', edge.id);
		if (edge.id.startsWith('emit-')) return;
		selectedEdgeId = edge.id;
		edges = edges.map((e) => ({ ...e, selected: e.id === edge.id }));
	}

	// Click on canvas background deselects
	function handlePaneClick() {
		selectedEdgeId = null;
		edges = edges.map((e) => ({ ...e, selected: false }));
	}

	async function deleteEdge(edgeId: string) {
		try {
			await webapi.delete(`/api/v1/commander/edges/${edgeId}`);
		} catch (err) {
			console.error('Failed to delete edge:', err);
		}
		edges = edges.filter((e) => e.id !== edgeId);
		selectedEdgeId = null;
	}

	// Keyboard: Backspace/Delete removes selected edge
	function handleKeydown(e: KeyboardEvent) {
		if ((e.key === 'Backspace' || e.key === 'Delete') && selectedEdgeId) {
			e.preventDefault();
			deleteEdge(selectedEdgeId);
		}
	}

	function scheduleSavePositions() {
		if (saveTimeout) clearTimeout(saveTimeout);
		saveTimeout = setTimeout(savePositions, 500);
	}

	async function savePositions() {
		const positions = nodes
			.filter((n) => n.type !== 'team')
			.map((n) => ({
				nodeId: n.id,
				x: n.position.x,
				y: n.position.y,
			}));
		try {
			await webapi.put('/api/v1/commander/layout', { positions });
		} catch (err) {
			console.error('Failed to save positions:', err);
		}
	}

	onMount(() => {
		loadGraph();

		const ws = getWebSocketClient();

		const unsubs = [
			ws.on('agent_activated', () => loadGraph()),
			ws.on('agent_deactivated', () => loadGraph()),
			ws.on('workflow_run_started', (data: any) => {
				if (!data?.roleId) return;
				nodes = nodes.map((n) =>
					n.id === data.roleId
						? { ...n, data: { ...n.data, status: 'running' } }
						: n
				);
			}),
			ws.on('workflow_run_completed', (data: any) => {
				if (!data?.roleId) return;
				nodes = nodes.map((n) =>
					n.id === data.roleId
						? { ...n, data: { ...n.data, status: 'active' } }
						: n
				);
			}),
			ws.on('workflow_run_failed', (data: any) => {
				if (!data?.roleId) return;
				nodes = nodes.map((n) =>
					n.id === data.roleId
						? { ...n, data: { ...n.data, status: 'active' } }
						: n
				);
			}),
		];

		return () => {
			for (const unsub of unsubs) {
				if (typeof unsub === 'function') unsub();
			}
			if (saveTimeout) clearTimeout(saveTimeout);
		};
	});
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="commander-canvas-wrapper">
	{#if loading}
		<div class="flex items-center justify-center h-full">
			<div class="loading loading-spinner loading-lg"></div>
		</div>
	{:else}
		<SvelteFlow
			{nodes}
			{edges}
			{nodeTypes}
			{edgeTypes}
			fitView
			minZoom={0.2}
			maxZoom={2}
			onnodeclick={handleNodeClick}
			onnodedragstop={handleNodeDragStop}
			onconnect={handleConnect}
			onedgeclick={handleEdgeClick}
			onpaneclick={handlePaneClick}
			defaultEdgeOptions={{ type: 'default', selectable: true }}
			proOptions={{ hideAttribution: true }}
			nodesConnectable={true}
			edgesFocusable={true}
			connectionMode="strict"
			deleteKeyCode="Backspace"
		>
			<Background />
			<CommanderToolbar
				onAutoLayout={handleAutoLayout}
				onDeleteSelected={() => { if (selectedEdgeId) deleteEdge(selectedEdgeId); }}
				hasSelection={selectedEdgeId !== null}
			/>
		</SvelteFlow>

		<AgentNodeDetail
			node={detailOpen ? selectedNode : null}
			onclose={() => { detailOpen = false; selectedNode = null; }}
		/>
	{/if}
</div>
