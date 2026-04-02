<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { getAgent, updateAgent } from '$lib/api/nebo';
	import RichInput from '$lib/components/ui/RichInput.svelte';
	import TagInput from '$lib/components/ui/TagInput.svelte';
	import { Undo2, Redo2, Plus, Trash2 } from 'lucide-svelte';
	import { t } from 'svelte-i18n';

	const channelState = getContext<{
		activeAgentId: string;
		activeAgentName: string;
	}>('channelState');

	type PropEntry = { key: string; text: string; tags: string[]; isArray: boolean };

	let loading = $state(true);
	let saving = $state(false);
	let personaMdValue = $state('');
	let savedValue = $state('');
	let properties = $state<PropEntry[]>([]);
	let savedProperties = $state('');

	// Undo/redo history (body text only)
	let undoStack = $state<string[]>([]);
	let redoStack = $state<string[]>([]);
	let historyTimer: ReturnType<typeof setTimeout> | null = null;

	const hasChanges = $derived(
		personaMdValue !== savedValue || JSON.stringify(properties) !== savedProperties
	);
	const canUndo = $derived(undoStack.length > 0);
	const canRedo = $derived(redoStack.length > 0);

	/** Convert API response properties to our local format. */
	function toEntries(items: { key: string; value: string | string[] }[]): PropEntry[] {
		return items.map((p) => {
			if (Array.isArray(p.value)) {
				return { key: p.key, text: '', tags: [...p.value], isArray: true };
			}
			return { key: p.key, text: String(p.value), tags: [], isArray: false };
		});
	}

	/** Reassemble agent_md from properties + body text. */
	function assembleMd(props: PropEntry[], body: string): string {
		const valid = props.filter((p) => p.key.trim());
		if (valid.length === 0) return body;

		let md = '---\n';
		for (const p of valid) {
			const k = p.key.trim();
			if (p.isArray) {
				md += `${k}:\n`;
				for (const item of p.tags) {
					md += `  - "${item}"\n`;
				}
			} else {
				md += `${k}: "${p.text.trim()}"\n`;
			}
		}
		md += '---\n\n';
		md += body;
		return md;
	}

	async function load() {
		loading = true;
		try {
			const res = await getAgent(channelState.activeAgentId);
			if (res?.agent) {
				// Use pre-parsed data from backend
				const apiProps = (res as any).personaProperties || [];
				const apiBody = (res as any).personaBody ?? '';

				properties = toEntries(apiProps);
				personaMdValue = apiBody;
				savedValue = personaMdValue;
				savedProperties = JSON.stringify(properties);
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

	function addProperty() {
		properties = [...properties, { key: '', text: '', tags: [], isArray: false }];
	}

	function removeProperty(index: number) {
		properties = properties.filter((_, i) => i !== index);
	}

	function updatePropertyKey(index: number, newKey: string) {
		const old = properties[index];
		const shouldBeArray = newKey.trim() === 'skills';

		if (shouldBeArray && !old.isArray) {
			const tags = old.text ? old.text.split(',').map((s) => s.trim()).filter(Boolean) : [];
			properties[index] = { key: newKey, text: '', tags, isArray: true };
		} else if (!shouldBeArray && old.isArray) {
			properties[index] = { key: newKey, text: old.tags.join(', '), tags: [], isArray: false };
		} else {
			properties[index] = { ...old, key: newKey };
		}
	}

	async function handleSave() {
		saving = true;
		try {
			const agentMd = assembleMd(properties, personaMdValue);
			await updateAgent(channelState.activeAgentId, { agentMd });
			savedValue = personaMdValue;
			savedProperties = JSON.stringify(properties);
		} finally {
			saving = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head>
	<title>Nebo - {channelState.activeAgentName || $t('agent.persona')} - {$t('agent.persona')}</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 flex flex-col min-h-0 max-w-3xl w-full mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<!-- Properties section -->
			<div class="mb-4 shrink-0">
				<div class="flex items-center justify-between mb-2 min-h-8">
					<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentPersona.properties')}</h2>
					<button
						type="button"
						class="btn btn-xs btn-ghost gap-1 text-base-content/60 hover:text-base-content"
						onclick={addProperty}
					>
						<Plus class="w-3.5 h-3.5" />
						{$t('agentPersona.addProperty')}
					</button>
				</div>
				{#if properties.length === 0}
					<p class="text-xs text-base-content/50 py-2">{$t('agentPersona.propertiesEmpty')}</p>
				{:else}
					<div class="flex flex-col gap-2">
						{#each properties as prop, i}
							<div class="flex items-start gap-2">
								<input
									type="text"
									class="input input-bordered input-sm w-36 shrink-0"
									value={prop.key}
									placeholder={$t('agentPersona.propertyName')}
									oninput={(e) => updatePropertyKey(i, (e.target as HTMLInputElement).value)}
								/>
								{#if prop.isArray}
									<div class="flex-1 min-w-0">
										<TagInput bind:value={properties[i].tags} placeholder={$t('agentPersona.propertyValue')} />
									</div>
								{:else}
									<input
										type="text"
										class="input input-bordered input-sm flex-1 min-w-0"
										bind:value={properties[i].text}
										placeholder={$t('agentPersona.propertyValue')}
									/>
								{/if}
								<button
									type="button"
									class="btn btn-xs btn-ghost btn-square text-base-content/40 hover:text-error mt-0.5"
									onclick={() => removeProperty(i)}
									title={$t('agentPersona.removeProperty', { values: { key: prop.key || '...' } })}
								>
									<Trash2 class="w-3.5 h-3.5" />
								</button>
							</div>
						{/each}
					</div>
				{/if}
			</div>

			<!-- Persona text editor -->
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
					placeholder={$t('agentPersona.personaPlaceholder')}
					onchange={(val) => handleChange(val)}
				/>
			</div>
		{/if}
	</div>
</div>
