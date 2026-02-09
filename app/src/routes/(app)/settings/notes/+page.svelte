<script lang="ts">
	import { onMount } from 'svelte';
	import { StickyNote, Plus, Check, AlertCircle } from 'lucide-svelte';
	import { getAgentProfile, updateAgentProfile, getSystemInfo } from '$lib/api/nebo';
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

	let isLoading = $state(true);
	let sections = $state<Section[]>([]);
	let systemSection = $state<Section | null>(null);
	let addingSectionName = $state('');
	let showAddSection = $state(false);
	let saveStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;
	let statusTimer: ReturnType<typeof setTimeout> | null = null;
	let initialized = $state(false);

	onMount(async () => {
		try {
			// Load profile and system info in parallel (system info is optional)
			const [profileData, sysInfo] = await Promise.all([
				getAgentProfile(),
				getSystemInfo().catch(() => null)
			]);

			// Build auto-discovered system section (always present, locked)
			const sysItems: RuleItem[] = [];
			if (sysInfo?.os)
				sysItems.push({ id: 'sys-os', text: `OS: ${sysInfo.os} (${sysInfo.arch})`, enabled: true });
			if (sysInfo?.hostname)
				sysItems.push({ id: 'sys-host', text: `Hostname: ${sysInfo.hostname}`, enabled: true });
			if (sysInfo?.homeDir)
				sysItems.push({ id: 'sys-home', text: `Home: ${sysInfo.homeDir}`, enabled: true });
			if (sysInfo?.username)
				sysItems.push({ id: 'sys-user', text: `User: ${sysInfo.username}`, enabled: true });
			systemSection = { id: 'system', name: 'System', items: sysItems };

			// Parse stored notes
			const raw = profileData?.toolNotes || '';
			const parsed = parseStructured(raw);
			sections = parsed.sections;
		} catch (err) {
			console.error('Failed to load notes:', err);
			sections = [];
		} finally {
			isLoading = false;
			setTimeout(() => (initialized = true), 100);
		}
	});

	function parseStructured(raw: string): StructuredData {
		if (!raw || !raw.trim()) {
			return { version: 1, sections: [] };
		}
		try {
			const parsed = JSON.parse(raw);
			if (parsed.version && parsed.sections) return parsed;
		} catch {}
		return { version: 1, sections: [] };
	}

	function autoSave() {
		if (!initialized) return;
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(async () => {
			saveStatus = 'saving';
			try {
				const data: StructuredData = { version: 1, sections };
				await updateAgentProfile({ toolNotes: JSON.stringify(data) });
				saveStatus = 'saved';
				if (statusTimer) clearTimeout(statusTimer);
				statusTimer = setTimeout(() => (saveStatus = 'idle'), 2000);
			} catch (err) {
				console.error('Failed to save notes:', err);
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
</script>

<div class="flex flex-col h-full min-h-0">
	<div class="shrink-0 mb-4">
		<div class="flex items-center gap-3 mb-2">
			<div class="w-10 h-10 rounded-xl bg-warning/10 flex items-center justify-center">
				<StickyNote class="w-5 h-5 text-warning" />
			</div>
			<div class="flex-1">
				<h2 class="text-lg font-semibold text-base-content">Tool Notes</h2>
				<p class="text-sm text-base-content/60">Environment context for your agent's tools</p>
			</div>
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
		</div>
	</div>

	{#if isLoading}
		<div class="flex-1 flex flex-col items-center justify-center gap-4">
			<Spinner size={32} />
			<p class="text-sm text-base-content/60">Loading notes...</p>
		</div>
	{:else}
		<div class="flex-1 space-y-3 min-h-0 overflow-y-auto">
			<!-- Auto-discovered System section (readonly, always first) -->
			{#if systemSection}
				<RuleSection
					section={systemSection}
					onUpdate={() => {}}
					onDelete={() => {}}
					readonly={true}
					itemLabel="note"
				/>
			{/if}

			<!-- User sections -->
			{#each sections as section (section.id)}
				<RuleSection
					{section}
					onUpdate={handleUpdateSection}
					onDelete={handleDeleteSection}
					itemLabel="note"
				/>
			{/each}

			{#if sections.length === 0 && !showAddSection}
				<div class="rounded-xl border border-dashed border-base-300 px-6 py-8 text-center">
					<p class="text-sm text-base-content/40 mb-3">
						Add sections like SSH Hosts, Development, Devices — anything your agent should know about your environment.
					</p>
					<button
						type="button"
						class="btn btn-ghost btn-sm text-primary"
						onclick={() => (showAddSection = true)}
					>
						<Plus class="w-4 h-4 mr-1" />
						Add your first section
					</button>
				</div>
			{/if}

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
