<script lang="ts">
	import { onMount } from 'svelte';
	import {
		MessagesSquare,
		Plus,
		Trash2,
		ChevronDown,
		ChevronUp,
		Brain,
		Clock,
		ArrowUpDown
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { AdvisorItem } from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let advisors = $state<AdvisorItem[]>([]);
	let error = $state('');
	let success = $state('');
	let showCreate = $state(false);
	let expandedAdvisor = $state<string | null>(null);
	let deleteConfirm = $state<string | null>(null);

	// Create form state
	let newName = $state('');
	let newRole = $state('general');
	let newDescription = $state('');
	let newPriority = $state(10);
	let newTimeout = $state(30);
	let newMemoryAccess = $state(false);
	let newPersona = $state('');
	let isCreating = $state(false);

	const timeoutOptions = [
		{ value: 10, label: '10s' },
		{ value: 15, label: '15s' },
		{ value: 30, label: '30s' },
		{ value: 45, label: '45s' },
		{ value: 60, label: '60s' }
	];

	const roleOptions = ['general', 'critic', 'builder', 'historian', 'strategist', 'analyst', 'innovator', 'user-advocate'];

	// Role → color mapping (subtle left border + pill only, neutral card bg)
	const roleColors: Record<string, { border: string; pill: string }> = {
		critic: { border: 'border-l-error', pill: 'bg-error/15 text-error' },
		builder: { border: 'border-l-success', pill: 'bg-success/15 text-success' },
		historian: { border: 'border-l-warning', pill: 'bg-warning/15 text-warning' },
		strategist: { border: 'border-l-info', pill: 'bg-info/15 text-info' },
		innovator: { border: 'border-l-secondary', pill: 'bg-secondary/15 text-secondary' },
		analyst: { border: 'border-l-accent', pill: 'bg-accent/15 text-accent' },
		'user-advocate': { border: 'border-l-primary', pill: 'bg-primary/15 text-primary' },
		general: { border: 'border-l-base-content/20', pill: 'bg-base-content/10 text-base-content/70' }
	};

	function getRoleStyle(role: string) {
		return roleColors[role] || roleColors.general;
	}

	onMount(async () => {
		await loadAdvisors();
	});

	async function loadAdvisors() {
		try {
			const res = await api.listAdvisors();
			advisors = res.advisors || [];
		} catch (err: any) {
			error = err?.message || 'Failed to load advisors';
		} finally {
			isLoading = false;
		}
	}

	async function handleCreate() {
		if (!newName.trim() || !newPersona.trim()) return;
		isCreating = true;
		error = '';
		try {
			const res = await api.createAdvisor({
				name: newName.trim().toLowerCase().replace(/\s+/g, '-'),
				role: newRole,
				description: newDescription,
				priority: newPriority,
				timeoutSeconds: newTimeout,
				memoryAccess: newMemoryAccess,
				persona: newPersona
			});
			advisors = [...advisors, res.advisor].sort((a, b) => b.priority - a.priority);
			resetCreateForm();
			showCreate = false;
			showSuccess('Advisor created');
		} catch (err: any) {
			error = err?.message || 'Failed to create advisor';
		} finally {
			isCreating = false;
		}
	}

	async function handleToggle(advisor: AdvisorItem) {
		try {
			const res = await api.updateAdvisor({ enabled: !advisor.enabled }, advisor.name);
			advisors = advisors.map((a) => (a.name === advisor.name ? res.advisor : a));
		} catch (err: any) {
			error = err?.message || 'Failed to update advisor';
		}
	}

	async function handleUpdate(advisor: AdvisorItem, field: string, value: any) {
		try {
			const res = await api.updateAdvisor({ [field]: value }, advisor.name);
			advisors = advisors.map((a) => (a.name === advisor.name ? res.advisor : a));
		} catch (err: any) {
			error = err?.message || 'Failed to update advisor';
		}
	}

	async function handleDelete(name: string) {
		try {
			await api.deleteAdvisor(name);
			advisors = advisors.filter((a) => a.name !== name);
			deleteConfirm = null;
			showSuccess('Advisor deleted');
		} catch (err: any) {
			error = err?.message || 'Failed to delete advisor';
		}
	}

	function showSuccess(msg: string) {
		success = msg;
		setTimeout(() => (success = ''), 3000);
	}

	function resetCreateForm() {
		newName = '';
		newRole = 'general';
		newDescription = '';
		newPriority = 10;
		newTimeout = 30;
		newMemoryAccess = false;
		newPersona = '';
	}

	function toggleExpand(name: string) {
		expandedAdvisor = expandedAdvisor === name ? null : name;
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Advisors</h2>
		<p class="text-sm text-base-content/70">
			Internal voices that deliberate before the agent responds
		</p>
	</div>
	<button
		class="h-9 px-4 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all flex items-center gap-1.5"
		onclick={() => (showCreate = !showCreate)}
	>
		<Plus class="w-4 h-4" />
		New Advisor
	</button>
</div>

{#if success}
	<div class="mb-4 rounded-xl bg-success/10 border border-success/20 px-4 py-3 text-sm text-success">
		{success}
	</div>
{/if}
{#if error}
	<div class="mb-4 rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-sm text-error">
		{error}
	</div>
{/if}

<!-- Create Form -->
{#if showCreate}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-4 mb-6">
		<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider">Create Advisor</h3>
		<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
			<div>
				<label class="text-sm font-medium text-base-content/70" for="advisor-name">
					Name (slug)
				</label>
				<input
					id="advisor-name"
					type="text"
					bind:value={newName}
					placeholder="e.g. skeptic"
					class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
				/>
			</div>
			<div>
				<label class="text-sm font-medium text-base-content/70" for="advisor-role">
					Role
				</label>
				<select
					id="advisor-role"
					bind:value={newRole}
					class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
				>
					{#each roleOptions as role}
						<option value={role}>{role}</option>
					{/each}
				</select>
			</div>
			<div>
				<label class="text-sm font-medium text-base-content/70" for="advisor-priority">
					Priority (higher = speaks first)
				</label>
				<input
					id="advisor-priority"
					type="number"
					bind:value={newPriority}
					min="1"
					max="100"
					class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
				/>
			</div>
			<div>
				<label class="text-sm font-medium text-base-content/70" for="advisor-timeout">
					Timeout
				</label>
				<select
					id="advisor-timeout"
					bind:value={newTimeout}
					class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
				>
					{#each timeoutOptions as opt}
						<option value={opt.value}>{opt.label}</option>
					{/each}
				</select>
			</div>
		</div>
		<div>
			<label class="text-sm font-medium text-base-content/70" for="advisor-description">
				Description
			</label>
			<input
				id="advisor-description"
				type="text"
				bind:value={newDescription}
				placeholder="What does this advisor do?"
				class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
			/>
		</div>
		<div class="flex items-center gap-3">
			<input
				id="advisor-memory"
				type="checkbox"
				bind:checked={newMemoryAccess}
				class="toggle toggle-sm toggle-primary"
			/>
			<label class="text-sm font-medium text-base-content/70 cursor-pointer" for="advisor-memory">
				Memory Access
			</label>
		</div>
		<div>
			<label class="text-sm font-medium text-base-content/70" for="advisor-persona">
				Persona (system prompt)
			</label>
			<textarea
				id="advisor-persona"
				bind:value={newPersona}
				placeholder="You are the Skeptic. Your role is to challenge ideas and find flaws..."
				class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-sm focus:outline-none focus:border-primary/50 transition-colors resize-none h-32"
			></textarea>
		</div>
		<div class="flex justify-end gap-2 pt-1">
			<button
				class="h-9 px-4 rounded-full text-sm font-medium text-base-content/70 hover:bg-base-content/5 transition-colors"
				onclick={() => {
					showCreate = false;
					resetCreateForm();
				}}
			>
				Cancel
			</button>
			<button
				class="h-9 px-5 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all disabled:opacity-30 flex items-center gap-1.5"
				onclick={handleCreate}
				disabled={isCreating || !newName.trim() || !newPersona.trim()}
			>
				{#if isCreating}
					<Spinner size={14} />
					Creating...
				{:else}
					Create
				{/if}
			</button>
		</div>
	</div>
{/if}

<!-- Advisor List -->
{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-sm text-base-content/70">Loading advisors...</span>
	</div>
{:else if advisors.length === 0}
	<div class="flex flex-col items-center justify-center py-16 gap-3 text-base-content/70">
		<MessagesSquare class="w-12 h-12" />
		<p class="font-medium">No advisors configured</p>
		<p class="text-sm">Advisors provide internal perspectives before the agent responds</p>
		<button
			class="h-9 px-4 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all flex items-center gap-1.5 mt-2"
			onclick={() => (showCreate = true)}
		>
			<Plus class="w-4 h-4" />
			Create your first advisor
		</button>
	</div>
{:else}
	<div class="space-y-3">
		{#each advisors as advisor (advisor.name)}
			{@const style = getRoleStyle(advisor.role)}
			<div
				class="rounded-2xl border-l-4 {style.border} bg-base-200/50 border border-base-content/10 transition-all"
				class:opacity-50={!advisor.enabled}
			>
				<!-- Advisor card -->
				<button
					class="w-full p-4 cursor-pointer text-left"
					onclick={() => toggleExpand(advisor.name)}
				>
					<div class="flex items-start gap-3">
						<!-- Toggle -->
						<div class="pt-0.5">
							<input
								type="checkbox"
								checked={advisor.enabled}
								onclick={(e) => e.stopPropagation()}
								onchange={() => handleToggle(advisor)}
								class="toggle toggle-sm toggle-primary"
							/>
						</div>

						<!-- Name + meta -->
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2 mb-1">
								<span class="font-semibold text-base-content">{advisor.name}</span>
								<span class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-semibold {style.pill}">{advisor.role}</span>
								{#if advisor.memoryAccess}
									<Brain class="w-3.5 h-3.5 text-primary" title="Has memory access" />
								{/if}
							</div>
							{#if advisor.description}
								<p class="text-sm text-base-content/70 leading-snug">
									{advisor.description}
								</p>
							{/if}
							<div class="flex items-center gap-3 mt-2 text-sm text-base-content/70">
								<span class="flex items-center gap-1">
									<Clock class="w-3 h-3" />
									{advisor.timeoutSeconds}s
								</span>
								<span class="flex items-center gap-1">
									<ArrowUpDown class="w-3 h-3" />
									Priority {advisor.priority}
								</span>
							</div>
						</div>

						<!-- Expand chevron -->
						<div class="pt-1">
							{#if expandedAdvisor === advisor.name}
								<ChevronUp class="w-4 h-4 text-base-content/70" />
							{:else}
								<ChevronDown class="w-4 h-4 text-base-content/70" />
							{/if}
						</div>
					</div>
				</button>

				<!-- Expanded details -->
				{#if expandedAdvisor === advisor.name}
					<div class="px-4 pb-4 space-y-4 border-t border-base-content/10">
						<div class="grid grid-cols-1 md:grid-cols-3 gap-4 pt-4">
							<div>
								<label class="text-sm font-medium text-base-content/70" for="edit-role-{advisor.name}">
									Role
								</label>
								<select
									id="edit-role-{advisor.name}"
									value={advisor.role}
									onchange={(e) => handleUpdate(advisor, 'role', e.currentTarget.value)}
									class="w-full h-9 mt-1.5 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
								>
									{#each roleOptions as role}
										<option value={role}>{role}</option>
									{/each}
								</select>
							</div>
							<div>
								<label class="text-sm font-medium text-base-content/70" for="edit-timeout-{advisor.name}">
									Timeout
								</label>
								<select
									id="edit-timeout-{advisor.name}"
									value={advisor.timeoutSeconds}
									onchange={(e) => handleUpdate(advisor, 'timeoutSeconds', parseInt(e.currentTarget.value))}
									class="w-full h-9 mt-1.5 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
								>
									{#each timeoutOptions as opt}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							</div>
							<div>
								<label class="text-sm font-medium text-base-content/70" for="edit-priority-{advisor.name}">
									Priority
								</label>
								<input
									id="edit-priority-{advisor.name}"
									type="number"
									value={advisor.priority}
									min="1"
									max="100"
									onchange={(e) => handleUpdate(advisor, 'priority', parseInt(e.currentTarget.value))}
									class="w-full h-9 mt-1.5 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
								/>
							</div>
						</div>

						<div>
							<label class="text-sm font-medium text-base-content/70" for="edit-desc-{advisor.name}">
								Description
							</label>
							<input
								id="edit-desc-{advisor.name}"
								type="text"
								value={advisor.description}
								onchange={(e) => handleUpdate(advisor, 'description', e.currentTarget.value)}
								class="w-full h-9 mt-1.5 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
							/>
						</div>

						<div class="flex items-center gap-3">
							<input
								id="edit-memory-{advisor.name}"
								type="checkbox"
								checked={advisor.memoryAccess}
								onchange={() => handleUpdate(advisor, 'memoryAccess', !advisor.memoryAccess)}
								class="toggle toggle-sm toggle-primary"
							/>
							<label class="text-sm font-medium text-base-content/70 cursor-pointer" for="edit-memory-{advisor.name}">
								Memory Access
							</label>
						</div>

						<div>
							<label class="text-sm font-medium text-base-content/70" for="edit-persona-{advisor.name}">
								Persona
							</label>
							<textarea
								id="edit-persona-{advisor.name}"
								value={advisor.persona}
								onchange={(e) => handleUpdate(advisor, 'persona', e.currentTarget.value)}
								class="w-full mt-1.5 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-sm font-mono leading-relaxed focus:outline-none focus:border-primary/50 transition-colors resize-none h-40"
							></textarea>
						</div>

						<div class="flex justify-end pt-1">
							{#if deleteConfirm === advisor.name}
								<div class="flex items-center gap-2">
									<span class="text-sm text-error">Delete this advisor?</span>
									<button
										class="h-7 px-3 rounded-full bg-error text-error-content text-xs font-bold hover:brightness-110 transition-all"
										onclick={() => handleDelete(advisor.name)}
									>
										Confirm
									</button>
									<button
										class="h-7 px-3 rounded-full text-xs font-medium text-base-content/70 hover:bg-base-content/5 transition-colors"
										onclick={() => (deleteConfirm = null)}
									>
										Cancel
									</button>
								</div>
							{:else}
								<button
									class="h-7 px-3 rounded-full text-xs font-medium text-base-content/70 hover:text-error hover:bg-error/10 transition-colors flex items-center gap-1"
									onclick={() => (deleteConfirm = advisor.name)}
								>
									<Trash2 class="w-3.5 h-3.5" />
									Delete
								</button>
							{/if}
						</div>
					</div>
				{/if}
			</div>
		{/each}
	</div>
{/if}
