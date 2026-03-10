<!--
  ToolTester — Execute registered tools directly and view results.
  Power-user tool testing interface for Nebo app developers.
-->

<script lang="ts">
	import { Wrench, RotateCcw, Trash2, Play, Loader2, ChevronDown } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import { generateUUID } from '$lib/utils';

	interface Props {
		appId: string;
	}

	let { appId }: Props = $props();

	interface HistoryEntry {
		id: string;
		tool: string;
		input: string;
		content: string;
		isError: boolean;
		timestamp: Date;
	}

	let tools = $state<components.ToolDefinitionItem[]>([]);
	let loadingTools = $state(false);
	let loadError = $state('');
	let selectedToolName = $state('');
	let inputJSON = $state('{}');
	let executing = $state(false);
	let history = $state<HistoryEntry[]>([]);
	let expandedEntryId = $state<string | null>(null);

	const selectedTool = $derived(tools.find((t) => t.name === selectedToolName) ?? null);

	const schemaString = $derived(
		selectedTool?.schema ? JSON.stringify(selectedTool.schema, null, 2) : ''
	);

	$effect(() => {
		loadTools();
	});

	async function loadTools() {
		loadingTools = true;
		loadError = '';
		try {
			const res = await api.listTools();
			tools = res.tools ?? [];
			if (!selectedToolName && tools.length > 0) {
				selectTool(tools[0].name);
			}
		} catch (err: any) {
			loadError = err.message || 'Failed to load tools';
		} finally {
			loadingTools = false;
		}
	}

	function selectTool(name: string) {
		selectedToolName = name;
		const tool = tools.find((t) => t.name === name);
		if (tool?.schema?.properties) {
			const stub: Record<string, any> = {};
			for (const [key, prop] of Object.entries(
				tool.schema.properties as Record<string, any>
			)) {
				if (prop.default !== undefined) {
					stub[key] = prop.default;
				}
			}
			inputJSON = Object.keys(stub).length > 0 ? JSON.stringify(stub, null, 2) : '{}';
		} else {
			inputJSON = '{}';
		}
	}

	async function executeTool() {
		if (!selectedToolName) return;
		let parsedInput: any;
		try {
			parsedInput = JSON.parse(inputJSON);
		} catch {
			history = [
				{
					id: generateUUID(),
					tool: selectedToolName,
					input: inputJSON,
					content: 'Invalid JSON input',
					isError: true,
					timestamp: new Date()
				},
				...history
			];
			return;
		}

		executing = true;
		try {
			const res = await api.toolExecute({
				tool: selectedToolName,
				input: parsedInput
			});
			history = [
				{
					id: generateUUID(),
					tool: selectedToolName,
					input: inputJSON,
					content: res.content,
					isError: res.isError,
					timestamp: new Date()
				},
				...history
			];
		} catch (err: any) {
			history = [
				{
					id: generateUUID(),
					tool: selectedToolName,
					input: inputJSON,
					content: err.message || 'Request failed',
					isError: true,
					timestamp: new Date()
				},
				...history
			];
		} finally {
			executing = false;
		}
	}

	function clearHistory() {
		history = [];
	}

	function toggleEntry(id: string) {
		expandedEntryId = expandedEntryId === id ? null : id;
	}

	function handleKeydown(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
			e.preventDefault();
			executeTool();
		}
	}
</script>

<div class="flex flex-col h-full">
	<!-- Toolbar -->
	<div
		class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-100"
	>
		<select
			bind:value={selectedToolName}
			onchange={() => selectTool(selectedToolName)}
			class="select select-bordered select-xs flex-shrink-0 max-w-[200px]"
		>
			{#if tools.length === 0}
				<option value="">No tools available</option>
			{/if}
			{#each tools as tool}
				<option value={tool.name}>{tool.name}</option>
			{/each}
		</select>

		<span class="text-xs text-base-content/40 flex-1 truncate">
			{selectedTool?.description ?? ''}
		</span>

		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={loadTools}
			title="Reload tools"
		>
			<RotateCcw class="w-3.5 h-3.5" />
		</button>

		{#if history.length > 0}
			<button
				type="button"
				class="btn btn-xs btn-ghost"
				onclick={clearHistory}
				title="Clear history"
			>
				<Trash2 class="w-3.5 h-3.5" />
			</button>
		{/if}
	</div>

	{#if loadingTools && tools.length === 0}
		<div class="flex flex-col items-center justify-center flex-1 gap-2">
			<Loader2 class="w-6 h-6 text-base-content/40 animate-spin" />
			<p class="text-xs text-base-content/50">Loading tools...</p>
		</div>
	{:else if loadError}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Wrench class="w-8 h-8" />
			<p class="text-sm text-error">{loadError}</p>
			<button type="button" class="btn btn-xs btn-ghost" onclick={loadTools}>Retry</button>
		</div>
	{:else if !selectedTool}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Wrench class="w-8 h-8" />
			<p class="text-sm font-medium">No Tools Available</p>
			<p class="text-xs">Sideload an app with tools to get started</p>
		</div>
	{:else}
		<div class="flex-1 min-h-0 flex flex-col">
			<!-- Input Section -->
			<div class="shrink-0 border-b border-base-300 p-3 space-y-2">
				{#if schemaString}
					<details class="text-xs">
						<summary
							class="cursor-pointer text-base-content/50 hover:text-base-content/70 select-none"
						>
							Schema reference
						</summary>
						<pre
							class="mt-1 p-2 bg-base-200 rounded-lg overflow-auto max-h-40 text-xs font-mono">{schemaString}</pre>
					</details>
				{/if}

				<textarea
					bind:value={inputJSON}
					class="textarea textarea-bordered w-full font-mono text-xs leading-relaxed"
					rows="5"
					placeholder={'{"resource": "...", "action": "..."}'}
					onkeydown={handleKeydown}
				></textarea>

				<div class="flex items-center justify-between">
					<span class="text-xs text-base-content/40">
						{navigator?.userAgent?.includes('Mac') ? 'Cmd' : 'Ctrl'}+Enter to run
					</span>
					<button
						type="button"
						class="btn btn-sm btn-primary"
						onclick={executeTool}
						disabled={executing || !selectedToolName}
					>
						{#if executing}
							<Loader2 class="w-4 h-4 animate-spin" />
							Running...
						{:else}
							<Play class="w-4 h-4" />
							Execute
						{/if}
					</button>
				</div>
			</div>

			<!-- History -->
			<div class="flex-1 min-h-0 overflow-y-auto p-3 space-y-2">
				{#if history.length === 0}
					<div
						class="flex flex-col items-center justify-center h-full text-base-content/30 gap-1"
					>
						<p class="text-xs">No executions yet</p>
					</div>
				{:else}
					{#each history as entry (entry.id)}
						<div
							class="rounded-lg border {entry.isError
								? 'border-error/30 bg-error/5'
								: 'border-base-300 bg-base-200/50'}"
						>
							<button
								type="button"
								class="w-full flex items-center gap-2 px-3 py-2 text-left"
								onclick={() => toggleEntry(entry.id)}
							>
								<span
									class="w-1.5 h-1.5 rounded-full shrink-0 {entry.isError
										? 'bg-error'
										: 'bg-success'}"
								></span>
								<span class="text-xs font-mono font-medium flex-1 truncate"
									>{entry.tool}</span
								>
								<span class="text-xs text-base-content/40">
									{entry.timestamp.toLocaleTimeString()}
								</span>
								<ChevronDown
									class="w-3 h-3 text-base-content/40 transition-transform {expandedEntryId ===
									entry.id
										? 'rotate-180'
										: ''}"
								/>
							</button>

							{#if expandedEntryId === entry.id}
								<div class="px-3 pb-3 space-y-2 border-t border-base-300/50">
									<div>
										<p class="text-xs text-base-content/50 mb-1">Input</p>
										<pre
											class="text-xs font-mono bg-base-300/50 rounded p-2 overflow-auto max-h-24">{entry.input}</pre>
									</div>
									<div>
										<p class="text-xs text-base-content/50 mb-1">Output</p>
										<pre
											class="text-xs font-mono bg-base-300/50 rounded p-2 overflow-auto max-h-48 whitespace-pre-wrap">{entry.content}</pre>
									</div>
								</div>
							{/if}
						</div>
					{/each}
				{/if}
			</div>
		</div>
	{/if}
</div>
