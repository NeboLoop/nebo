<script lang="ts">
	import { goto } from '$app/navigation';
	import { Bot, MessageSquare, Store } from 'lucide-svelte';
	import type { ActiveRoleEntry, SimpleAgentStatusResponse } from '$lib/api/neboComponents';

	let {
		roles = [],
		agentStatus = null,
		isLoading = true
	}: {
		roles: ActiveRoleEntry[];
		agentStatus: SimpleAgentStatusResponse | null;
		isLoading: boolean;
	} = $props();

	let connected = $derived(agentStatus?.connected ?? false);
</script>

{#if isLoading}
	<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
		{#each Array(3) as _}
			<div class="card bg-base-200 border border-base-300">
				<div class="card-body p-5">
					<div class="skeleton h-4 w-24 mb-2"></div>
					<div class="skeleton h-3 w-32 mb-3"></div>
					<div class="skeleton h-8 w-20"></div>
				</div>
			</div>
		{/each}
	</div>
{:else}
	<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
		<!-- Companion (always present) -->
		<div class="card bg-base-200 border border-base-300 hover:border-base-content/40 transition-all duration-200">
			<div class="card-body p-5">
				<div class="flex items-center gap-3 mb-3">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
						<Bot class="w-5 h-5 text-primary" />
					</div>
					<div class="flex-1 min-w-0">
						<div class="font-bold text-base truncate">Companion</div>
						<div class="flex items-center gap-1.5 mt-0.5">
							<span class="dashboard-status-dot {connected ? 'bg-success' : 'bg-error'}"></span>
							<span class="text-sm text-base-content/60">{connected ? 'Online' : 'Offline'}</span>
						</div>
					</div>
				</div>
				<button class="btn btn-primary btn-sm btn-outline gap-1.5" onclick={() => goto('/agents')}>
					<MessageSquare class="w-3.5 h-3.5" /> Chat
				</button>
			</div>
		</div>

		<!-- Active roles -->
		{#each roles as role (role.roleId)}
			<div class="card bg-base-200 border border-base-300 hover:border-base-content/40 transition-all duration-200">
				<div class="card-body p-5">
					<div class="flex items-center gap-3 mb-3">
						<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
							<Bot class="w-5 h-5 text-secondary" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="font-bold text-base truncate">{role.name}</div>
							<div class="flex items-center gap-1.5 mt-0.5">
								<span class="dashboard-status-dot bg-success"></span>
								<span class="text-sm text-base-content/60">Active</span>
							</div>
						</div>
					</div>
					<div class="flex items-center gap-2">
						<button class="btn btn-secondary btn-sm btn-outline gap-1.5" onclick={() => goto(`/agent/role/${role.roleId}/chat`)}>
							<MessageSquare class="w-3.5 h-3.5" /> Chat
						</button>
					</div>
				</div>
			</div>
		{/each}

		<!-- Empty state hint -->
		{#if roles.length === 0}
			<div class="card bg-base-200 border border-base-300 border-dashed">
				<div class="card-body p-5 items-center justify-center text-center">
					<Store class="w-6 h-6 text-base-content/60 mb-1" />
					<p class="text-sm text-base-content/80">
						<a href="/marketplace" class="link link-primary">Install roles from the Marketplace</a>
					</p>
				</div>
			</div>
		{/if}
	</div>
{/if}
