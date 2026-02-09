<script lang="ts">
	import { onMount } from 'svelte';
	import { ScrollText, RotateCcw, Plus, Check, AlertCircle } from 'lucide-svelte';
	import { getAgentProfile, updateAgentProfile } from '$lib/api/nebo';
	import { generateUUID } from '$lib/utils';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import RuleSection from '$lib/components/settings/RuleSection.svelte';

	interface RuleItem {
		id: string;
		text: string;
		enabled: boolean;
	}

	interface Section {
		id: string;
		name: string;
		items: RuleItem[];
	}

	interface StructuredData {
		version: number;
		sections: Section[];
	}

	const DEFAULT_RULES: StructuredData = {
		version: 1,
		sections: [
			{
				id: 'safety',
				name: 'Safety',
				items: [
					{
						id: 's1',
						text: 'Never share my personal information with others',
						enabled: true
					},
					{
						id: 's2',
						text: 'Ask before taking destructive actions (deleting files, sending messages)',
						enabled: true
					},
					{
						id: 's3',
						text: 'Always confirm before spending money or making purchases',
						enabled: true
					},
					{ id: 's4', text: 'When in doubt, ask before acting externally', enabled: true }
				]
			},
			{
				id: 'communication',
				name: 'Communication',
				items: [
					{
						id: 'c1',
						text: "Be direct — don't pad responses with unnecessary preamble",
						enabled: true
					},
					{
						id: 'c2',
						text: 'Skip the "Great question!" and "I\'d be happy to help!" — just help',
						enabled: true
					},
					{ id: 'c3', text: 'If you disagree with me, say so', enabled: true },
					{
						id: 'c4',
						text: 'Keep responses focused unless I ask for detail',
						enabled: true
					},
					{ id: 'c5', text: "Have opinions — don't be a yes-machine", enabled: true }
				]
			},
			{
				id: 'memory',
				name: 'Memory',
				items: [
					{
						id: 'm1',
						text: 'Remember my preferences without being told twice',
						enabled: true
					},
					{
						id: 'm2',
						text: 'Don\'t announce that you\'re "remembering" things — just do it',
						enabled: true
					},
					{ id: 'm3', text: 'Forget something if I ask you to', enabled: true }
				]
			},
			{
				id: 'proactivity',
				name: 'Proactivity',
				items: [
					{
						id: 'p1',
						text: 'Check in on ongoing projects without being asked',
						enabled: true
					},
					{ id: 'p2', text: 'Flag deadlines and commitments', enabled: true },
					{
						id: 'p3',
						text: 'Suggest improvements when you notice patterns',
						enabled: true
					}
				]
			}
		]
	};

	let isLoading = $state(true);
	let sections = $state<Section[]>([]);
	let addingSectionName = $state('');
	let showAddSection = $state(false);
	let saveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;
	let statusTimer: ReturnType<typeof setTimeout> | null = null;
	let initialized = $state(false);

	onMount(async () => {
		try {
			const data = await getAgentProfile();
			const raw = data?.agentRules || '';
			const parsed = parseStructured(raw);
			sections = parsed.sections;
		} catch (err) {
			console.error('Failed to load rules:', err);
			sections = structuredClone(DEFAULT_RULES.sections);
		} finally {
			isLoading = false;
			// Delay enabling auto-save so initial load doesn't trigger it
			setTimeout(() => (initialized = true), 100);
		}
	});

	function parseStructured(raw: string): StructuredData {
		if (!raw || !raw.trim()) {
			return structuredClone(DEFAULT_RULES);
		}
		try {
			const parsed = JSON.parse(raw);
			if (parsed.version && parsed.sections) return parsed;
		} catch {}
		return structuredClone(DEFAULT_RULES);
	}

	function autoSave() {
		if (!initialized) return;
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(async () => {
			saveStatus = 'saving';
			try {
				const data: StructuredData = { version: 1, sections };
				await updateAgentProfile({ agentRules: JSON.stringify(data) });
				saveStatus = 'saved';
				if (statusTimer) clearTimeout(statusTimer);
				statusTimer = setTimeout(() => (saveStatus = 'idle'), 2000);
			} catch (err) {
				console.error('Failed to save rules:', err);
				saveStatus = 'error';
				if (statusTimer) clearTimeout(statusTimer);
				statusTimer = setTimeout(() => (saveStatus = 'idle'), 4000);
			}
		}, 500);
	}

	function handleUpdateSection(updated: Section) {
		sections = sections.map((s) => (s.id === updated.id ? updated : s));
		autoSave();
	}

	function handleDeleteSection(sectionId: string) {
		sections = sections.filter((s) => s.id !== sectionId);
		autoSave();
	}

	function addSection() {
		if (!addingSectionName.trim()) return;
		const newSection: Section = {
			id: generateUUID(),
			name: addingSectionName.trim(),
			items: []
		};
		sections = [...sections, newSection];
		addingSectionName = '';
		showAddSection = false;
		autoSave();
	}

	function handleAddSectionKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') addSection();
		if (e.key === 'Escape') {
			showAddSection = false;
			addingSectionName = '';
		}
	}

	function handleReset() {
		sections = structuredClone(DEFAULT_RULES.sections);
		autoSave();
	}
</script>

<div class="flex flex-col h-full min-h-0">
	<div class="shrink-0 mb-4">
		<div class="flex items-center gap-3 mb-2">
			<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
				<ScrollText class="w-5 h-5 text-primary" />
			</div>
			<div class="flex-1">
				<h2 class="text-lg font-semibold text-base-content">Agent Rules</h2>
				<p class="text-sm text-base-content/60">Define how your agent should behave</p>
			</div>
			<div class="flex items-center gap-2">
				{#if saveStatus === 'saving'}
					<span class="flex items-center gap-1.5 text-xs text-base-content/40">
						<Spinner size={12} />
						Saving...
					</span>
				{:else if saveStatus === 'saved'}
					<span class="flex items-center gap-1.5 text-xs text-success">
						<Check class="w-3.5 h-3.5" />
						Saved
					</span>
				{:else if saveStatus === 'error'}
					<span class="flex items-center gap-1.5 text-xs text-error">
						<AlertCircle class="w-3.5 h-3.5" />
						Failed to save
					</span>
				{/if}
				<button
					type="button"
					class="text-base-content/30 hover:text-base-content/60 transition-colors"
					title="Reset to defaults"
					onclick={handleReset}
				>
					<RotateCcw class="w-4 h-4" />
				</button>
			</div>
		</div>
	</div>

	{#if isLoading}
		<div class="flex-1 flex flex-col items-center justify-center gap-4">
			<Spinner size={32} />
			<p class="text-sm text-base-content/60">Loading rules...</p>
		</div>
	{:else}
		<div class="flex-1 space-y-3 min-h-0 overflow-y-auto">
			{#each sections as section (section.id)}
				<RuleSection
					{section}
					onUpdate={handleUpdateSection}
					onDelete={handleDeleteSection}
					itemLabel="rule"
				/>
			{/each}

			{#if showAddSection}
				<div class="flex items-center gap-2 px-1">
					<!-- svelte-ignore a11y_autofocus -->
					<input
						class="input input-bordered input-sm flex-1"
						placeholder="Section name..."
						bind:value={addingSectionName}
						onkeydown={handleAddSectionKeydown}
						onblur={() => {
							if (addingSectionName.trim()) addSection();
							else showAddSection = false;
						}}
						autofocus
					/>
				</div>
			{:else}
				<button
					type="button"
					class="flex items-center gap-2 text-sm text-base-content/40 hover:text-primary transition-colors px-1 py-2"
					onclick={() => (showAddSection = true)}
				>
					<Plus class="w-4 h-4" />
					Add Section
				</button>
			{/if}
		</div>
	{/if}
</div>
