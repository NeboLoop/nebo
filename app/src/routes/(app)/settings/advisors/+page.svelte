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
	import Button from '$lib/components/ui/Button.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
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

	// Role → color mapping for visual distinction
	const roleColors: Record<string, { border: string; badge: string; bg: string }> = {
		critic: { border: 'border-l-error', badge: 'badge-error', bg: 'bg-error/5' },
		builder: { border: 'border-l-success', badge: 'badge-success', bg: 'bg-success/5' },
		historian: { border: 'border-l-warning', badge: 'badge-warning', bg: 'bg-warning/5' },
		strategist: { border: 'border-l-info', badge: 'badge-info', bg: 'bg-info/5' },
		innovator: { border: 'border-l-secondary', badge: 'badge-secondary', bg: 'bg-secondary/5' },
		analyst: { border: 'border-l-accent', badge: 'badge-accent', bg: 'bg-accent/5' },
		'user-advocate': { border: 'border-l-primary', badge: 'badge-primary', bg: 'bg-primary/5' },
		general: { border: 'border-l-base-content/20', badge: 'badge-ghost', bg: 'bg-base-200' }
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

<div class="flex flex-col gap-5">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-3">
			<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
				<MessagesSquare class="w-5 h-5 text-secondary" />
			</div>
			<div>
				<h2 class="text-lg font-semibold text-base-content">Advisors</h2>
				<p class="text-sm text-base-content/60">
					Internal voices that deliberate before the agent responds
				</p>
			</div>
		</div>
		<Button type="primary" size="sm" onclick={() => (showCreate = !showCreate)}>
			<Plus class="w-4 h-4 mr-1" />
			New Advisor
		</Button>
	</div>

	<!-- Alerts -->
	{#if success}
		<Alert type="success">{success}</Alert>
	{/if}
	{#if error}
		<Alert type="error" title="Error">{error}</Alert>
	{/if}

	<!-- Create Form -->
	{#if showCreate}
		<div class="bg-base-200 rounded-xl p-5 space-y-4 border border-base-300">
			<h3 class="font-semibold text-base-content">Create Advisor</h3>
			<div class="grid grid-cols-1 md:grid-cols-2 gap-3">
				<div>
					<label class="label" for="advisor-name">
						<span class="label-text">Name (slug)</span>
					</label>
					<input
						id="advisor-name"
						type="text"
						bind:value={newName}
						placeholder="e.g. skeptic"
						class="input input-bordered input-sm w-full"
					/>
				</div>
				<div>
					<label class="label" for="advisor-role">
						<span class="label-text">Role</span>
					</label>
					<select id="advisor-role" bind:value={newRole} class="select select-bordered select-sm w-full">
						{#each roleOptions as role}
							<option value={role}>{role}</option>
						{/each}
					</select>
				</div>
				<div>
					<label class="label" for="advisor-priority">
						<span class="label-text">Priority (higher = speaks first)</span>
					</label>
					<input
						id="advisor-priority"
						type="number"
						bind:value={newPriority}
						min="1"
						max="100"
						class="input input-bordered input-sm w-full"
					/>
				</div>
				<div>
					<label class="label" for="advisor-timeout">
						<span class="label-text">Timeout</span>
					</label>
					<select
						id="advisor-timeout"
						bind:value={newTimeout}
						class="select select-bordered select-sm w-full"
					>
						{#each timeoutOptions as opt}
							<option value={opt.value}>{opt.label}</option>
						{/each}
					</select>
				</div>
			</div>
			<div>
				<label class="label" for="advisor-description">
					<span class="label-text">Description</span>
				</label>
				<input
					id="advisor-description"
					type="text"
					bind:value={newDescription}
					placeholder="What does this advisor do?"
					class="input input-bordered input-sm w-full"
				/>
			</div>
			<div>
				<label class="label cursor-pointer justify-start gap-3" for="advisor-memory">
					<input
						id="advisor-memory"
						type="checkbox"
						bind:checked={newMemoryAccess}
						class="toggle toggle-sm toggle-primary"
					/>
					<span class="label-text">Memory Access</span>
				</label>
			</div>
			<div>
				<label class="label" for="advisor-persona">
					<span class="label-text">Persona (system prompt)</span>
				</label>
				<textarea
					id="advisor-persona"
					bind:value={newPersona}
					placeholder="You are the Skeptic. Your role is to challenge ideas and find flaws..."
					class="textarea textarea-bordered w-full h-32 text-sm"
				></textarea>
			</div>
			<div class="flex justify-end gap-2 pt-1">
				<Button
					type="ghost"
					size="sm"
					onclick={() => {
						showCreate = false;
						resetCreateForm();
					}}
				>
					Cancel
				</Button>
				<Button
					type="primary"
					size="sm"
					onclick={handleCreate}
					disabled={isCreating || !newName.trim() || !newPersona.trim()}
				>
					{#if isCreating}
						<Spinner size={14} />
						<span class="ml-1">Creating...</span>
					{:else}
						Create
					{/if}
				</Button>
			</div>
		</div>
	{/if}

	<!-- Advisor List -->
	{#if isLoading}
		<div class="flex flex-col items-center justify-center py-12 gap-4">
			<Spinner size={32} />
			<p class="text-sm text-base-content/60">Loading advisors...</p>
		</div>
	{:else if advisors.length === 0}
		<div class="flex flex-col items-center justify-center py-16 gap-3 text-base-content/50">
			<MessagesSquare class="w-12 h-12" />
			<p class="font-medium">No advisors configured</p>
			<p class="text-sm">Advisors provide internal perspectives before the agent responds</p>
			<Button type="primary" size="sm" onclick={() => (showCreate = true)}>
				<Plus class="w-4 h-4 mr-1" />
				Create your first advisor
			</Button>
		</div>
	{:else}
		<div class="space-y-3">
			{#each advisors as advisor (advisor.name)}
				{@const style = getRoleStyle(advisor.role)}
				<div
					class="rounded-xl border-l-4 {style.border} {style.bg} transition-all"
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
									<span class="badge badge-sm {style.badge}">{advisor.role}</span>
									{#if advisor.memoryAccess}
										<Brain class="w-3.5 h-3.5 text-primary" title="Has memory access" />
									{/if}
								</div>
								{#if advisor.description}
									<p class="text-sm text-base-content/60 leading-snug">
										{advisor.description}
									</p>
								{/if}
								<div class="flex items-center gap-3 mt-2 text-xs text-base-content/40">
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
									<ChevronUp class="w-4 h-4 text-base-content/40" />
								{:else}
									<ChevronDown class="w-4 h-4 text-base-content/40" />
								{/if}
							</div>
						</div>
					</button>

					<!-- Expanded details -->
					{#if expandedAdvisor === advisor.name}
						<div class="px-4 pb-4 space-y-3 border-t border-base-300/50">
							<div class="grid grid-cols-1 md:grid-cols-3 gap-3 pt-3">
								<div>
									<label class="label" for="edit-role-{advisor.name}">
										<span class="label-text text-xs">Role</span>
									</label>
									<select
										id="edit-role-{advisor.name}"
										value={advisor.role}
										onchange={(e) => handleUpdate(advisor, 'role', e.currentTarget.value)}
										class="select select-bordered select-sm w-full"
									>
										{#each roleOptions as role}
											<option value={role}>{role}</option>
										{/each}
									</select>
								</div>
								<div>
									<label class="label" for="edit-timeout-{advisor.name}">
										<span class="label-text text-xs">Timeout</span>
									</label>
									<select
										id="edit-timeout-{advisor.name}"
										value={advisor.timeoutSeconds}
										onchange={(e) => handleUpdate(advisor, 'timeoutSeconds', parseInt(e.currentTarget.value))}
										class="select select-bordered select-sm w-full"
									>
										{#each timeoutOptions as opt}
											<option value={opt.value}>{opt.label}</option>
										{/each}
									</select>
								</div>
								<div>
									<label class="label" for="edit-priority-{advisor.name}">
										<span class="label-text text-xs">Priority</span>
									</label>
									<input
										id="edit-priority-{advisor.name}"
										type="number"
										value={advisor.priority}
										min="1"
										max="100"
										onchange={(e) => handleUpdate(advisor, 'priority', parseInt(e.currentTarget.value))}
										class="input input-bordered input-sm w-full"
									/>
								</div>
							</div>

							<div>
								<label class="label" for="edit-desc-{advisor.name}">
									<span class="label-text text-xs">Description</span>
								</label>
								<input
									id="edit-desc-{advisor.name}"
									type="text"
									value={advisor.description}
									onchange={(e) => handleUpdate(advisor, 'description', e.currentTarget.value)}
									class="input input-bordered input-sm w-full"
								/>
							</div>

							<label class="label cursor-pointer justify-start gap-3" for="edit-memory-{advisor.name}">
								<input
									id="edit-memory-{advisor.name}"
									type="checkbox"
									checked={advisor.memoryAccess}
									onchange={() => handleUpdate(advisor, 'memoryAccess', !advisor.memoryAccess)}
									class="toggle toggle-sm toggle-primary"
								/>
								<span class="label-text text-xs">Memory Access</span>
							</label>

							<div>
								<label class="label" for="edit-persona-{advisor.name}">
									<span class="label-text text-xs">Persona</span>
								</label>
								<textarea
									id="edit-persona-{advisor.name}"
									value={advisor.persona}
									onchange={(e) => handleUpdate(advisor, 'persona', e.currentTarget.value)}
									class="textarea textarea-bordered w-full h-40 text-sm font-mono leading-relaxed"
								></textarea>
							</div>

							<div class="flex justify-end pt-1">
								{#if deleteConfirm === advisor.name}
									<div class="flex items-center gap-2">
										<span class="text-xs text-error">Delete this advisor?</span>
										<Button type="error" size="xs" onclick={() => handleDelete(advisor.name)}>
											Confirm
										</Button>
										<Button type="ghost" size="xs" onclick={() => (deleteConfirm = null)}>
											Cancel
										</Button>
									</div>
								{:else}
									<Button
										type="ghost"
										size="xs"
										onclick={() => (deleteConfirm = advisor.name)}
									>
										<Trash2 class="w-3.5 h-3.5 mr-1 text-error" />
										Delete
									</Button>
								{/if}
							</div>
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
