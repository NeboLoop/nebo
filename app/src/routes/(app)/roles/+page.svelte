<script lang="ts">
	import { onMount } from 'svelte';
	import { UserCircle, Power, Trash2, RefreshCw, Loader2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	interface InstalledRole {
		id: string;
		name: string;
		description: string;
		version: string;
		enabled: boolean;
		source: 'marketplace' | 'user';
	}

	let roles = $state<InstalledRole[]>([]);
	let isLoading = $state(true);

	onMount(async () => {
		await loadRoles();
	});

	async function loadRoles() {
		isLoading = true;
		try {
			const data = await api.listRoles();
			roles = data.roles || [];
		} catch (err) {
			console.error('Failed to load roles:', err);
		} finally {
			isLoading = false;
		}
	}
</script>

<div class="max-w-4xl mx-auto">
	<div class="flex items-center justify-between mb-6">
		<div>
			<h1 class="font-display text-2xl font-bold text-base-content mb-1">Roles</h1>
			<p class="text-base text-base-content/80">Manage installed agent roles and their configurations</p>
		</div>
		<button
			type="button"
			class="h-9 px-4 rounded-xl bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-2"
			onclick={loadRoles}
		>
			<RefreshCw class="w-4 h-4" />
			Refresh
		</button>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-base text-base-content/80">Loading roles...</span>
		</div>
	{:else if roles.length === 0}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-12 text-center">
			<UserCircle class="w-10 h-10 mx-auto mb-3 text-base-content/60" />
			<h3 class="font-display font-bold text-base-content mb-1">No roles installed</h3>
			<p class="text-base text-base-content/80 mb-4">
				Browse the marketplace to find and install agent roles
			</p>
			<a
				href="/marketplace/roles"
				class="inline-flex h-9 px-5 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all items-center"
			>
				Browse Roles
			</a>
		</div>
	{:else}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
			{#each roles as role}
				<div class="flex items-center gap-4 px-5 py-4">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
						<UserCircle class="w-5 h-5 text-primary" />
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2">
							<span class="text-base font-medium text-base-content">{role.name}</span>
							<span class="text-sm font-medium text-base-content/80 bg-base-content/5 px-1.5 py-0.5 rounded">v{role.version}</span>
							{#if role.source === 'user'}
								<span class="text-sm font-medium text-info bg-info/10 px-1.5 py-0.5 rounded">Local</span>
							{/if}
						</div>
						{#if role.description}
							<p class="text-base text-base-content/80 mt-0.5 truncate">{role.description}</p>
						{/if}
					</div>
					<span class="text-base {role.enabled ? 'text-success' : 'text-base-content/80'}">
						{role.enabled ? 'Active' : 'Disabled'}
					</span>
				</div>
			{/each}
		</div>
	{/if}
</div>
