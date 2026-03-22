<script lang="ts">
	import { onMount } from 'svelte';
	import { getAgentProfile, updateAgentProfile } from '$lib/api/nebo';
	import RichInput from '$lib/components/ui/RichInput.svelte';
	import { Undo2, Redo2 } from 'lucide-svelte';

	let loading = $state(true);
	let saving = $state(false);
	let roleMdValue = $state('');
	let savedValue = $state('');

	// Undo/redo history
	let undoStack = $state<string[]>([]);
	let redoStack = $state<string[]>([]);
	let historyTimer: ReturnType<typeof setTimeout> | null = null;

	const hasChanges = $derived(roleMdValue !== savedValue);
	const canUndo = $derived(undoStack.length > 0);
	const canRedo = $derived(redoStack.length > 0);

	async function load() {
		loading = true;
		try {
			const profile = await getAgentProfile();
			if (profile) {
				roleMdValue = profile.customPersonality || '';
				savedValue = roleMdValue;
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}
	}

	function handleChange(val: string) {
		const prev = roleMdValue;
		roleMdValue = val;

		if (historyTimer) clearTimeout(historyTimer);
		historyTimer = setTimeout(() => {
			if (prev !== val) {
				undoStack = [...undoStack, prev];
				redoStack = [];
			}
		}, 500);
	}

	function undo() {
		if (!canUndo) return;
		const prev = undoStack[undoStack.length - 1];
		undoStack = undoStack.slice(0, -1);
		redoStack = [...redoStack, roleMdValue];
		roleMdValue = prev;
	}

	function redo() {
		if (!canRedo) return;
		const next = redoStack[redoStack.length - 1];
		redoStack = redoStack.slice(0, -1);
		undoStack = [...undoStack, roleMdValue];
		roleMdValue = next;
	}

	async function handleSave() {
		saving = true;
		try {
			await updateAgentProfile({ customPersonality: roleMdValue });
			savedValue = roleMdValue;
		} finally {
			saving = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head>
	<title>Nebo - Assistant - Role</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 flex flex-col min-h-0 max-w-3xl w-full mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<div class="flex items-center justify-between mb-3 min-h-8 shrink-0">
				<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">Role</h2>
				<div class="flex items-center gap-1.5">
					<button
						type="button"
						class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-base-content/70"
						disabled={!canUndo}
						onclick={undo}
						title="Undo"
					>
						<Undo2 class="w-3.5 h-3.5" />
					</button>
					<button
						type="button"
						class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-base-content/70"
						disabled={!canRedo}
						onclick={redo}
						title="Redo"
					>
						<Redo2 class="w-3.5 h-3.5" />
					</button>
					<button
						class="btn btn-sm btn-primary"
						class:opacity-50={saving}
						disabled={saving || !hasChanges}
						onclick={handleSave}
					>
						{saving ? 'Saving...' : 'Save'}
					</button>
				</div>
			</div>
			<div class="flex-1 flex flex-col min-h-0 rich-input-expand">
				<RichInput
					bind:value={roleMdValue}
					mode="full"
					placeholder="Define the assistant's personality, role, and behavioral guidelines. e.g. You are a personal companion who helps manage daily tasks... Type / to mention an MCP, skill, or agent."
					onchange={(val) => handleChange(val)}
				/>
			</div>
		{/if}
	</div>
</div>
