<script lang="ts">
	import { onMount } from 'svelte';
	import { getAgentProfile, updateAgentProfile } from '$lib/api/nebo';
	import RichInput from '$lib/components/ui/RichInput.svelte';
	import { Undo2, Redo2 } from 'lucide-svelte';
	import { t } from 'svelte-i18n';

	let loading = $state(true);
	let saving = $state(false);
	let personaMdValue = $state('');
	let savedValue = $state('');

	// Undo/redo history
	let undoStack = $state<string[]>([]);
	let redoStack = $state<string[]>([]);
	let historyTimer: ReturnType<typeof setTimeout> | null = null;

	const hasChanges = $derived(personaMdValue !== savedValue);
	const canUndo = $derived(undoStack.length > 0);
	const canRedo = $derived(redoStack.length > 0);

	async function load() {
		loading = true;
		try {
			const profile = await getAgentProfile();
			if (profile) {
				personaMdValue = profile.customPersonality || '';
				savedValue = personaMdValue;
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}
	}

	function handleChange(val: string) {
		const prev = personaMdValue;
		personaMdValue = val;

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
		redoStack = [...redoStack, personaMdValue];
		personaMdValue = prev;
	}

	function redo() {
		if (!canRedo) return;
		const next = redoStack[redoStack.length - 1];
		redoStack = redoStack.slice(0, -1);
		undoStack = [...undoStack, personaMdValue];
		personaMdValue = next;
	}

	async function handleSave() {
		saving = true;
		try {
			await updateAgentProfile({ customPersonality: personaMdValue });
			savedValue = personaMdValue;
		} finally {
			saving = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head>
	<title>Nebo - Assistant - Persona</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 flex flex-col min-h-0 max-w-3xl w-full mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<div class="flex items-center justify-between mb-3 min-h-8 shrink-0">
				<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentPersona.title')}</h2>
				<div class="flex items-center gap-1.5">
					<button
						type="button"
						class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-base-content/70"
						disabled={!canUndo}
						onclick={undo}
						title={$t('agentPersona.undo')}
					>
						<Undo2 class="w-3.5 h-3.5" />
					</button>
					<button
						type="button"
						class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-base-content/70"
						disabled={!canRedo}
						onclick={redo}
						title={$t('agentPersona.redo')}
					>
						<Redo2 class="w-3.5 h-3.5" />
					</button>
					<button
						class="btn btn-sm btn-primary"
						class:opacity-50={saving}
						disabled={saving || !hasChanges}
						onclick={handleSave}
					>
						{saving ? $t('common.saving') : $t('common.save')}
					</button>
				</div>
			</div>
			<div class="flex-1 flex flex-col min-h-0 rich-input-expand">
				<RichInput
					bind:value={personaMdValue}
					mode="full"
					placeholder={$t('agentPersona.assistantPlaceholder')}
					onchange={(val) => handleChange(val)}
				/>
			</div>
		{/if}
	</div>
</div>
