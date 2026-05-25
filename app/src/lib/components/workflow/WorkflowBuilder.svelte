<script lang="ts">
	import BuilderCanvas from './BuilderCanvas.svelte';
	import NodeConfigPanel from './NodeConfigPanel.svelte';
	import NodeCatalog from './NodeCatalog.svelte';
	import BuilderChat from './BuilderChat.svelte';
	import {
		addActivityToWorkflow,
		removeActivityFromWorkflow,
		duplicateActivityInWorkflow,
		generateLinearConnections,
		removeConnection,
		type WorkflowConnection,
	} from '$lib/utils/workflowLayout';
	import { createTypedActivity, isBranchingType, getActivityType } from '$lib/utils/workflowTypes';
	import type { WorkflowConfig, WorkflowActivity, WorkflowTrigger } from '$lib/types/agentPage';
	import { untrack } from 'svelte';

	let {
		workflows = {},
		agentId = '',
		agentName = 'Agent',
		onclose,
		onsave,
	}: {
		workflows: Record<string, WorkflowConfig>;
		agentId: string;
		agentName: string;
		onclose?: () => void;
		onsave?: (workflows: Record<string, WorkflowConfig>) => void;
	} = $props();

	// ── Mutable builder state (deep clone from read-only props)
	const originalSnapshot = $derived(JSON.stringify(workflows));
	let builderWorkflows = $state<Record<string, WorkflowConfig>>(untrack(() => JSON.parse(JSON.stringify(workflows))));

	// ── Active workflow (single-workflow canvas)
	let activeWorkflowName = $state<string>(untrack(() => Object.keys(builderWorkflows)[0] || ''));
	const activeWorkflow = $derived(builderWorkflows[activeWorkflowName] ?? null);
	const workflowNames = $derived(Object.keys(builderWorkflows));

	// ── Dirty tracking
	const isDirty = $derived(JSON.stringify(builderWorkflows) !== originalSnapshot);

	// ── Undo / Redo history
	let undoStack = $state<string[]>(untrack(() => [originalSnapshot]));
	let undoPointer = $state(0);
	const canUndo = $derived(undoPointer > 0);
	const canRedo = $derived(undoPointer < undoStack.length - 1);

	function pushUndoSnapshot() {
		const snap = JSON.stringify(builderWorkflows);
		undoStack = [...undoStack.slice(0, undoPointer + 1), snap];
		undoPointer = undoStack.length - 1;
	}

	function undo() {
		if (!canUndo) return;
		undoPointer--;
		builderWorkflows = JSON.parse(undoStack[undoPointer]);
	}

	function redo() {
		if (!canRedo) return;
		undoPointer++;
		builderWorkflows = JSON.parse(undoStack[undoPointer]);
	}

	// ── Validation
	interface ValidationError { workflowName: string; nodeId?: string; message: string }
	const validationErrors = $derived.by<ValidationError[]>(() => {
		const errors: ValidationError[] = [];
		for (const [wfName, wf] of Object.entries(builderWorkflows)) {
			const acts = (wf as WorkflowConfig).activities || [];
			const ids = new Set<string>();
			for (const act of acts) {
				if (!act.id || !act.id.trim()) {
					errors.push({ workflowName: wfName, nodeId: act.id, message: `Activity in "${wfName}" has an empty ID` });
				} else if (ids.has(act.id)) {
					errors.push({ workflowName: wfName, nodeId: act.id, message: `Duplicate activity ID "${act.id}" in "${wfName}"` });
				}
				ids.add(act.id);
				if (!act.intent || !act.intent.trim()) {
					errors.push({ workflowName: wfName, nodeId: act.id, message: `Activity "${act.id}" in "${wfName}" has no intent` });
				}
			}
		}
		return errors;
	});
	const hasErrors = $derived(validationErrors.length > 0);

	// ── Save / Discard
	function handleSave() {
		if (hasErrors) return;
		onsave?.(JSON.parse(JSON.stringify(builderWorkflows)));
	}

	function handleDiscard() {
		builderWorkflows = JSON.parse(originalSnapshot);
		undoStack = [originalSnapshot];
		undoPointer = 0;
		selectedNodeId = null;
	}

	function handleClose() {
		if (isDirty) {
			if (!confirm('You have unsaved changes. Discard and close?')) return;
		}
		onclose?.();
	}

	// ── Selection
	let selectedNodeId = $state<string | null>(null);
	let mode = $state<'view' | 'edit'>('edit');

	// ── Panels
	let chatOpen = $state(true);
	let catalogOpen = $state(false);
	let catalogInsertAfter = $state<string | null>(null);
	let catalogInsertBranchLabel = $state<string | null>(null);

	// ── Confirm modal
	let confirmModal = $state<{
		type: 'node' | 'workflow';
		nodeId?: string;
		label: string;
	} | null>(null);

	// ── Derived
	const selectedActivity = $derived.by(() => {
		if (!activeWorkflow || !selectedNodeId) return null;
		return activeWorkflow.activities?.find((a: WorkflowActivity) => a.id === selectedNodeId) || null;
	});

	// ── Immutable workflow update helper (with undo snapshot)
	function updateActiveWorkflow(updater: (wf: WorkflowConfig) => WorkflowConfig) {
		const wf = builderWorkflows[activeWorkflowName];
		if (!wf) return;
		const updated = updater(JSON.parse(JSON.stringify(wf)));
		builderWorkflows = { ...builderWorkflows, [activeWorkflowName]: updated };
		pushUndoSnapshot();
	}

	// ── Node mutations
	function handleAddNode(catalogItem: Record<string, unknown>, afterNodeId: string | null, branchLabel?: string | null) {
		if (!activeWorkflowName || !builderWorkflows[activeWorkflowName]) return;
		const itemType = typeof catalogItem.type === 'string' ? catalogItem.type : '';

		updateActiveWorkflow((wf) => {
			if (itemType === 'emit') {
				wf.emit = 'new.event';
			} else if (itemType.startsWith('trigger-')) {
				const triggerType = itemType.replace('trigger-', '');
				wf.trigger = { type: triggerType };
			} else {
				const newAct = createTypedActivity(itemType, catalogItem as { label: string; desc: string; agentId?: string; serverId?: string; serverName?: string });
				wf.activities = addActivityToWorkflow(wf.activities || [], afterNodeId, newAct).map(a => ({ ...a, type: a.type || 'custom' }));

				const branching = isBranchingType(newAct.type);
				const typeDef = branching ? getActivityType(newAct.type) : null;
				const newBranchLabels = typeDef?.branchLabels ?? [];

				if (!wf.connections) {
					const existingActs = (wf.activities ?? []).filter((a: WorkflowActivity) => a.id !== newAct.id);
					wf.connections = generateLinearConnections(existingActs, wf.emit);
				}

				const conns: WorkflowConnection[] = [...wf.connections];

				if (branchLabel === '__parallel__' && afterNodeId) {
					conns.push({ from: afterNodeId, to: newAct.id });
				} else if (branchLabel && afterNodeId) {
					conns.push({ from: afterNodeId, to: newAct.id, label: branchLabel });
				} else if (afterNodeId) {
					const outgoing = conns.filter(c => c.from === afterNodeId);
					if (outgoing.length > 0) {
						const firstTarget = outgoing[0].to;
						const firstLabel = outgoing[0].label;
						const idx = conns.indexOf(outgoing[0]);
						conns.splice(idx, 1, { from: afterNodeId, to: newAct.id, ...(firstLabel ? { label: firstLabel } : {}) });
						conns.push({
							from: newAct.id,
							to: firstTarget,
							...(branching && newBranchLabels[0] ? { label: newBranchLabels[0] } : {}),
						});
					} else {
						conns.push({ from: afterNodeId, to: newAct.id });
					}
				} else {
					const acts = wf.activities ?? [];
					const prevAct = acts.length > 1 ? acts[acts.length - 2] : null;
					const from = prevAct ? prevAct.id : '__trigger__';
					conns.push({ from, to: newAct.id });
				}

				if (branching && newBranchLabels[0]) {
					const outgoing = conns.filter(c => c.from === newAct.id);
					if (outgoing.length > 0 && !outgoing[0].label) {
						outgoing[0].label = newBranchLabels[0];
					}
				}

				wf.connections = conns;
				selectedNodeId = newAct.id;
			}
			return wf;
		});

		catalogOpen = false;
		catalogInsertAfter = null;
		catalogInsertBranchLabel = null;
	}

	function handleRemoveNode(nodeId: string) {
		updateActiveWorkflow((wf) => {
			if (nodeId === '__emit__') {
				wf.emit = undefined;
			} else if (nodeId === '__trigger__') {
				wf.trigger = { type: 'manual' };
			} else {
				if (wf.connections) {
					const parents = wf.connections.filter((c: WorkflowConnection) => c.to === nodeId).map((c: WorkflowConnection) => c.from);
					const children = wf.connections.filter((c: WorkflowConnection) => c.from === nodeId).map((c: WorkflowConnection) => c.to);
					wf.connections = wf.connections.filter((c: WorkflowConnection) => c.from !== nodeId && c.to !== nodeId);
					for (const p of parents) {
						for (const ch of children) {
							wf.connections.push({ from: p, to: ch });
						}
					}
				}
				wf.activities = removeActivityFromWorkflow(wf.activities || [], nodeId).map(a => ({ ...a, type: a.type || 'custom' }));
			}
			return wf;
		});

		if (selectedNodeId === nodeId) selectedNodeId = null;
	}

	function handleConfirmRemoveNode(nodeId: string) {
		if (nodeId === '__trigger__') return;
		const label = nodeId === '__emit__' ? 'Emit' : nodeId;
		confirmModal = { type: 'node', nodeId, label };
	}

	function handleConfirmRemoveWorkflow() {
		const actCount = activeWorkflow?.activities?.length ?? 0;
		const label = `${activeWorkflowName} (${actCount} ${actCount === 1 ? 'activity' : 'activities'})`;
		confirmModal = { type: 'workflow', label };
	}

	function handleRemoveWorkflow() {
		const { [activeWorkflowName]: _, ...rest } = builderWorkflows;
		builderWorkflows = rest;
		pushUndoSnapshot();
		activeWorkflowName = Object.keys(builderWorkflows)[0] || '';
		selectedNodeId = null;
	}

	function executeConfirm() {
		if (!confirmModal) return;
		if (confirmModal.type === 'node' && confirmModal.nodeId) {
			handleRemoveNode(confirmModal.nodeId);
		} else if (confirmModal.type === 'workflow') {
			handleRemoveWorkflow();
		}
		confirmModal = null;
	}

	function handleDuplicateNode(nodeId: string) {
		if (nodeId === '__trigger__' || nodeId === '__emit__') return;

		updateActiveWorkflow((wf) => {
			const oldActivities = wf.activities || [];
			const newActivities = duplicateActivityInWorkflow(oldActivities, nodeId).map(a => ({ ...a, type: a.type || 'custom' }));
			wf.activities = newActivities;

			const origIdx = newActivities.findIndex((a: WorkflowActivity) => a.id === nodeId);
			if (origIdx >= 0 && origIdx + 1 < newActivities.length) {
				const dupeId = newActivities[origIdx + 1].id;

				if (wf.connections) {
					const outIdx = wf.connections.findIndex((c: WorkflowConnection) => c.from === nodeId);
					if (outIdx >= 0) {
						const target = wf.connections[outIdx].to;
						wf.connections[outIdx] = { from: nodeId, to: dupeId };
						wf.connections.push({ from: dupeId, to: target });
					} else {
						wf.connections.push({ from: nodeId, to: dupeId });
					}
				}

				selectedNodeId = dupeId;
			}
			return wf;
		});
	}

	function handleUpdateActivity(activityId: string, field: keyof WorkflowActivity, value: unknown) {
		updateActiveWorkflow((wf) => {
			const act = wf.activities?.find((a: WorkflowActivity) => a.id === activityId);
			if (!act) return wf;
			switch (field) {
				case 'id': act.id = value as string; break;
				case 'type': act.type = value as string; break;
				case 'label': act.label = value as string; break;
				case 'description': act.description = value as string; break;
				case 'tool': act.tool = value as string; break;
				case 'resource': act.resource = value as string; break;
				case 'action': act.action = value as string; break;
				case 'intent': act.intent = value as string; break;
				case 'skills': act.skills = value as string[]; break;
				case 'steps': act.steps = value as string[]; break;
				case 'params': act.params = value as Record<string, unknown>; break;
				case 'branches': act.branches = value as { label: string; nextId?: string }[]; break;
			}
			return wf;
		});
	}

	function handleUpdateTrigger(trigger: WorkflowTrigger) {
		updateActiveWorkflow((wf) => { wf.trigger = trigger; return wf; });
	}

	function handleUpdateEmit(emit: string) {
		updateActiveWorkflow((wf) => { wf.emit = emit || undefined; return wf; });
	}

	function handleUpdateDescription(desc: string) {
		updateActiveWorkflow((wf) => { wf.description = desc; return wf; });
	}

	function handleSelectNode(nodeId: string | null) {
		selectedNodeId = nodeId;
		if (catalogOpen) {
			catalogOpen = false;
			catalogInsertAfter = null;
			catalogInsertBranchLabel = null;
		}
	}

	function handleOpenCatalog(afterNodeId: string | null, branchLabel?: string) {
		catalogInsertAfter = afterNodeId;
		catalogInsertBranchLabel = branchLabel ?? null;
		catalogOpen = true;
	}

	function handleCreateConnection(fromId: string, toId: string) {
		updateActiveWorkflow((wf) => {
			if (!wf.connections) {
				wf.connections = generateLinearConnections(wf.activities || [], wf.emit);
			}
			const exists = wf.connections.some((c: WorkflowConnection) => c.from === fromId && c.to === toId);
			if (!exists) {
				wf.connections = [...wf.connections, { from: fromId, to: toId }];
			}
			return wf;
		});
	}

	function handleRemoveConnection(fromId: string, toId: string) {
		updateActiveWorkflow((wf) => {
			if (!wf.connections) return wf;
			wf.connections = removeConnection(wf.connections, fromId, toId);
			return wf;
		});
	}

	function handleDropNode(item: Record<string, unknown>, afterNodeId: string | null) {
		handleAddNode(item, afterNodeId);
	}

	interface ArchitectPayload {
		workflowName?: string;
		label?: string;
		type?: string;
		intent?: string;
		skills?: string[];
		steps?: string[];
		params?: Record<string, unknown>;
	}

	function handleArchitectAction(action: string, payload: unknown) {
		const p = payload as ArchitectPayload;
		if (action === 'add-activity' && p.workflowName) {
			const wfName = p.workflowName;
			if (!builderWorkflows[wfName]) return;
			if (wfName !== activeWorkflowName) activeWorkflowName = wfName;

			const newId = (p.label || 'new-step').toLowerCase().replace(/\s+/g, '-') + '-' + Date.now().toString(36);
			updateActiveWorkflow((wf) => {
				const newAct = {
					id: newId,
					type: p.type || 'custom',
					intent: p.intent || 'New activity',
					skills: p.skills || [],
					steps: p.steps || [],
					params: p.params || {},
				};
				wf.activities = addActivityToWorkflow(wf.activities || [], null, newAct).map(a => ({ ...a, type: a.type || 'custom' }));
				return wf;
			});
			selectedNodeId = newId;
		}
	}

	function handleNewWorkflow() {
		const name = `workflow-${Object.keys(builderWorkflows).length + 1}`;
		const newWf = {
			trigger: { type: 'manual' },
			description: 'New workflow',
			isActive: true,
			activities: [],
		};
		builderWorkflows = { ...builderWorkflows, [name]: newWf };
		pushUndoSnapshot();
		activeWorkflowName = name;
		selectedNodeId = null;
	}

	function handleTidyUp() {
		builderWorkflows = JSON.parse(JSON.stringify(builderWorkflows));
	}

	// ── Keyboard shortcuts
	function handleKeyboard(e: KeyboardEvent) {
		const meta = e.metaKey || e.ctrlKey;
		if (e.key === 'Escape' && catalogOpen) {
			catalogOpen = false;
			catalogInsertAfter = null;
			catalogInsertBranchLabel = null;
			return;
		}
		if (meta && e.key === 'z' && !e.shiftKey) {
			e.preventDefault();
			undo();
		} else if (meta && e.key === 'z' && e.shiftKey) {
			e.preventDefault();
			redo();
		} else if (meta && e.key === 's') {
			e.preventDefault();
			handleSave();
		}
	}
</script>

<svelte:window onkeydown={handleKeyboard} />

<div class="flex h-full w-full overflow-hidden">
	<!-- Left panel: AI Architect Chat -->
	{#if mode === 'edit' && chatOpen}
		<div class="w-[320px] shrink-0 border-r border-base-content/10 flex flex-col overflow-hidden">
			<BuilderChat
				{agentId}
				workflows={builderWorkflows}
				selectedWorkflowName={activeWorkflowName}
				selectedActivityId={selectedNodeId}
				onaction={handleArchitectAction}
			/>
		</div>
	{/if}

	<!-- Center: Canvas + Toolbar -->
	<div class="flex-1 min-w-0 flex flex-col overflow-hidden">
		<!-- Toolbar -->
		<div class="flex items-center gap-2 px-3 py-2 border-b border-base-content/10 shrink-0 bg-base-100">
			{#if mode === 'edit'}
				<!-- Chat toggle -->
				<button
					class="btn btn-sm btn-ghost gap-1.5 {chatOpen ? 'btn-active' : ''}"
					title="{chatOpen ? 'Hide' : 'Show'} Architect chat"
					onclick={() => chatOpen = !chatOpen}
				>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m3 21 1.9-5.7a8.5 8.5 0 1 1 3.8 3.8z"/></svg>
					<span class="text-xs">Architect</span>
				</button>

				<div class="w-px h-5 bg-base-content/10"></div>

				<!-- Undo -->
				<button class="btn btn-sm btn-ghost btn-square" title="Undo (Cmd+Z)" disabled={!canUndo} onclick={undo}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
				</button>
				<!-- Redo -->
				<button class="btn btn-sm btn-ghost btn-square" title="Redo (Cmd+Shift+Z)" disabled={!canRedo} onclick={redo}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.13-9.36L23 10"/></svg>
				</button>

				<div class="w-px h-5 bg-base-content/10"></div>

				<!-- Add node -->
				<button
					class="btn btn-sm btn-primary gap-1.5"
					onclick={() => handleOpenCatalog(null)}
				>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
					<span class="text-xs">Add Node</span>
				</button>

				<!-- Tidy up -->
				<button class="btn btn-sm btn-ghost gap-1.5" title="Auto-layout" onclick={handleTidyUp}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
					<span class="text-xs">Tidy Up</span>
				</button>

				<div class="w-px h-5 bg-base-content/10"></div>
			{/if}

			<div class="flex-1"></div>

			<!-- Validation errors -->
			{#if hasErrors && mode === 'edit'}
				<div class="flex items-center gap-1.5 text-xs text-warning" title={validationErrors.map(e => e.message).join('\n')}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
					<span>{validationErrors.length} {validationErrors.length === 1 ? 'issue' : 'issues'}</span>
				</div>
			{/if}

			<!-- Save / Discard -->
			{#if isDirty && mode === 'edit'}
				<button class="btn btn-sm btn-ghost text-xs" onclick={handleDiscard}>Discard</button>
				<button class="btn btn-sm btn-primary text-xs" disabled={hasErrors} onclick={handleSave}>Save</button>
			{/if}

			<!-- Mode toggle -->
			<div class="flex items-center rounded-lg border border-base-300 overflow-hidden">
				<button
					class="px-3 py-1 text-xs font-medium cursor-pointer border-none transition-colors
						{mode === 'view' ? 'bg-base-200 text-base-content' : 'bg-transparent text-base-content/50 hover:text-base-content/70'}"
					onclick={() => mode = 'view'}
				>View</button>
				<button
					class="px-3 py-1 text-xs font-medium cursor-pointer border-none transition-colors
						{mode === 'edit' ? 'bg-base-200 text-base-content' : 'bg-transparent text-base-content/50 hover:text-base-content/70'}"
					onclick={() => mode = 'edit'}
				>Edit</button>
			</div>
		</div>

		<!-- Workflow tabs -->
		{#if workflowNames.length > 0}
			<div class="flex items-center gap-0.5 px-2 py-1 border-b border-base-content/10 bg-base-200/50 shrink-0 overflow-x-auto">
				{#each workflowNames as wfName}
					<button
						class="px-3 py-1 text-xs font-medium rounded-md cursor-pointer border-none transition-colors shrink-0
							{wfName === activeWorkflowName ? 'bg-base-100 text-base-content shadow-sm' : 'bg-transparent text-base-content/50 hover:text-base-content/70 hover:bg-base-100/50'}"
						onclick={() => { activeWorkflowName = wfName; selectedNodeId = null; }}
					>{wfName}</button>
				{/each}
				<button
					class="px-2 py-1 text-xs text-base-content/40 hover:text-base-content/70 cursor-pointer border-none bg-transparent rounded-md hover:bg-base-100/50 transition-colors shrink-0"
					onclick={handleNewWorkflow}
					title="New workflow"
				>+</button>
			</div>
		{/if}

		<!-- Canvas -->
		<div class="flex-1 min-h-0 relative">
			{#if activeWorkflow}
				<BuilderCanvas
					workflow={activeWorkflow}
					workflowName={activeWorkflowName}
					{agentId}
					{mode}
					{selectedNodeId}
					onselect={handleSelectNode}
					onopenCatalog={handleOpenCatalog}
					onremove={handleConfirmRemoveNode}
					onduplicate={handleDuplicateNode}
					oncreateConnection={handleCreateConnection}
					onremoveConnection={handleRemoveConnection}
					ondropNode={handleDropNode}
				/>
			{:else}
				<div class="flex h-full items-center justify-center flex-col gap-3">
					<div class="text-3xl text-base-content/20">+</div>
					<span class="text-xs text-base-content/50">No workflows — create one to get started</span>
					<button class="btn btn-sm btn-primary" onclick={handleNewWorkflow}>New Workflow</button>
				</div>
			{/if}

			<!-- Node Catalog panel -->
			{#if catalogOpen}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="absolute right-0 top-0 bottom-0 z-[70] w-[300px] border-l border-base-content/10 bg-base-100 shadow-xl flex flex-col"
					ondragover={(e) => e.preventDefault()}
					ondrop={(e) => { e.preventDefault(); e.stopPropagation(); }}
				>
					<NodeCatalog
						onselect={(item) => handleAddNode(item, catalogInsertAfter, catalogInsertBranchLabel)}
						onclose={() => { catalogOpen = false; catalogInsertAfter = null; catalogInsertBranchLabel = null; }}
					/>
				</div>
			{/if}
		</div>
	</div>

	<!-- Right panel: Node Config -->
	{#if activeWorkflow}
		<NodeConfigPanel
			workflowName={activeWorkflowName}
			workflow={activeWorkflow}
			{selectedNodeId}
			activity={selectedActivity}
			{mode}
			onupdateActivity={(field, value) => {
				if (selectedNodeId) handleUpdateActivity(selectedNodeId, field, value);
			}}
			onupdateTrigger={(trigger) => handleUpdateTrigger(trigger)}
			onupdateEmit={(emit) => handleUpdateEmit(emit)}
			onupdateDescription={(desc) => handleUpdateDescription(desc)}
			onremove={(nodeId) => handleConfirmRemoveNode(nodeId)}
			onremoveWorkflow={() => handleConfirmRemoveWorkflow()}
			onclose={() => { selectedNodeId = null; }}
			onselectActivity={(id) => { selectedNodeId = id || null; }}
		/>
	{/if}
</div>

<!-- Confirm delete modal -->
{#if confirmModal}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="fixed inset-0 z-[100] flex items-center justify-center" onclick={() => confirmModal = null}>
		<div class="absolute inset-0 bg-black/40"></div>
		<div
			class="relative bg-base-100 rounded-xl border border-base-300 shadow-xl w-[400px] max-w-[90vw] p-6"
			onclick={(e) => e.stopPropagation()}
		>
			<div class="text-base font-semibold mb-2">
				{confirmModal.type === 'workflow' ? 'Delete Workflow' : 'Delete Node'}
			</div>
			<div class="text-sm text-base-content/70 mb-1">
				{#if confirmModal.type === 'workflow'}
					Are you sure you want to delete this workflow? This will remove all its activities and connections.
				{:else}
					Are you sure you want to delete this node? Its connections will be reconnected automatically.
				{/if}
			</div>
			<div class="text-xs font-mono text-base-content/50 bg-base-200 rounded px-2.5 py-1.5 mb-5 truncate">
				{confirmModal.label}
			</div>
			<div class="flex justify-end gap-2">
				<button
					class="btn btn-sm btn-ghost"
					onclick={() => confirmModal = null}
				>Cancel</button>
				<button
					class="btn btn-sm btn-error"
					onclick={executeConfirm}
				>Delete</button>
			</div>
		</div>
	</div>
{/if}
