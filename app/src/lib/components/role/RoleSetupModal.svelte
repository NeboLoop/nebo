<script lang="ts">
	import { installStoreApp, listRoles, activateRole, updateRoleWorkflow, getRoleWorkflows } from '$lib/api/nebo';
	import { X } from 'lucide-svelte';

	let {
		appId,
		roleName,
		roleDescription,
		inputs,
		onComplete,
		onCancel,
	}: {
		appId: string;
		roleName: string;
		roleDescription: string;
		inputs: Record<string, unknown>;
		onComplete: (roleId: string) => void;
		onCancel: () => void;
	} = $props();

	let values: Record<string, string> = $state({});
	let installing = $state(false);
	let error = $state('');

	// Pre-fill from defaults
	for (const [key, val] of Object.entries(inputs)) {
		values[key] = typeof val === 'string' ? val : JSON.stringify(val);
	}

	function prettifyKey(key: string): string {
		return key
			.replace(/[_-]/g, ' ')
			.replace(/\b\w/g, c => c.toUpperCase());
	}

	async function handleInstall() {
		installing = true;
		error = '';
		try {
			// 1. Install the app
			await installStoreApp(appId);

			// 2. Find the newly installed role
			const rolesRes = await listRoles();
			const allRoles = rolesRes?.roles || [];
			const matchedRole = allRoles.find(
				(r: any) => r.name?.toLowerCase() === roleName.toLowerCase()
			) || allRoles[allRoles.length - 1];

			if (!matchedRole) {
				error = 'Role installed but could not be found. Check the Agents panel.';
				installing = false;
				return;
			}

			const roleId = matchedRole.id;

			// 3. Update workflow inputs if any
			try {
				const wfRes = await getRoleWorkflows(roleId);
				if (wfRes?.workflows) {
					for (const wf of wfRes.workflows) {
						await updateRoleWorkflow(roleId, wf.bindingName, { inputs: values });
					}
				}
			} catch {
				// Non-critical — inputs can be configured later
			}

			// 4. Activate the role
			await activateRole(roleId);

			// 5. Done
			onComplete(roleId);
		} catch (e: any) {
			error = e?.error || e?.message || 'Failed to install role';
		} finally {
			installing = false;
		}
	}

	const inputKeys = $derived(Object.keys(inputs));
</script>

<div class="fixed inset-0 z-[60] flex items-center justify-center p-4 sm:p-8">
	<div class="absolute inset-0 bg-black/60 backdrop-blur-sm"></div>

	<div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden">
		{#if installing}
			<div class="flex flex-col items-center justify-center py-16 px-6">
				<span class="loading loading-spinner loading-lg text-primary"></span>
				<p class="text-base font-medium mt-4">Setting up {roleName}...</p>
				<p class="text-sm text-base-content/50 mt-1">This just takes a moment</p>
			</div>
		{:else}
			<!-- Header -->
			<div class="flex items-center justify-between px-6 pt-6 pb-2">
				<div></div>
				<button
					type="button"
					class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors"
					onclick={onCancel}
					aria-label="Close"
				>
					<X class="w-4 h-4 text-base-content/60" />
				</button>
			</div>

			<div class="px-6 pb-6">
				<!-- Title -->
				<div class="text-center mb-6">
					<h2 class="font-display text-xl font-bold">Set up {roleName}</h2>
					{#if roleDescription}
						<p class="text-sm text-base-content/60 mt-1 line-clamp-2">{roleDescription}</p>
					{/if}
				</div>

				{#if error}
					<div class="text-sm text-error bg-error/10 rounded-lg px-3 py-2 mb-4">{error}</div>
				{/if}

				{#if inputKeys.length > 0}
					<div class="border-t border-base-content/10 pt-4 mb-6">
						<p class="text-sm text-base-content/60 mb-4">
							Before {roleName} gets to work, tell it a bit about you.
						</p>
						<div class="flex flex-col gap-4">
							{#each inputKeys as key}
								<div>
									<label class="text-sm font-medium text-base-content/80 block mb-1.5" for="setup-{key}">
										{prettifyKey(key)}
									</label>
									{#if (values[key] || '').length > 60}
										<textarea
											id="setup-{key}"
											class="w-full rounded-lg bg-base-content/5 border border-base-content/10 px-3 py-2 text-sm focus:outline-none focus:border-primary/50 transition-colors resize-none"
											rows="2"
											bind:value={values[key]}
										></textarea>
									{:else}
										<input
											id="setup-{key}"
											type="text"
											class="w-full h-10 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
											bind:value={values[key]}
										/>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Actions -->
				<div class="flex gap-3">
					<button
						type="button"
						class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={onCancel}
					>
						Cancel
					</button>
					<button
						type="button"
						class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
						onclick={handleInstall}
					>
						Start working
					</button>
				</div>
			</div>
		{/if}
	</div>
</div>
