<script lang="ts">
	import { onMount, getContext } from 'svelte';
	import { getRole, updateRoleInputs, getEntityConfig, updateEntityConfig, pickFolder } from '$lib/api/nebo';
	import type { RoleInputField, ResolvedEntityConfig } from '$lib/api/neboComponents';
	import RoleInputForm from '$lib/components/agent/RoleInputForm.svelte';
	import { FolderOpen } from 'lucide-svelte';
	import { t } from 'svelte-i18n';

	const channelState = getContext<{
		activeRoleId: string;
		activeRoleName: string;
	}>('channelState');

	let loading = $state(true);
	let saving = $state(false);

	// Role inputs
	let inputFields = $state<RoleInputField[]>([]);
	let inputValues = $state<Record<string, unknown>>({});
	let savedInputValues = $state<string>('{}');

	// Entity config
	let entityConfig = $state<ResolvedEntityConfig | null>(null);

	// Allowed paths
	let allowedPathsText = $state('');
	let savedPathsText = $state('');

	// Detect Tauri (native app vs headless browser)
	let isTauri = $state(false);

	const folders = $derived(allowedPathsText.split('\n').filter(p => p.trim()));

	const hasChanges = $derived(
		JSON.stringify(inputValues) !== savedInputValues ||
		allowedPathsText !== savedPathsText
	);

	async function load() {
		loading = true;
		try {
			const [roleRes, configRes] = await Promise.all([
				getRole(channelState.activeRoleId).catch(() => null),
				getEntityConfig('role', channelState.activeRoleId).catch(() => null),
			]);

			if (roleRes?.role) {
				// Use pre-normalized inputFields from backend
				inputFields = roleRes.inputFields || [];
				try {
					inputValues = JSON.parse(roleRes.role.inputValues || '{}');
					savedInputValues = JSON.stringify(inputValues);
				} catch {
					inputValues = {};
					savedInputValues = '{}';
				}
			}

			if (configRes?.config) {
				entityConfig = configRes.config;
				allowedPathsText = (configRes.config.allowedPaths || []).join('\n');
				savedPathsText = allowedPathsText;
			}
		} catch {
			// ignore
		} finally {
			loading = false;
		}
	}

	function handleInputChange(values: Record<string, unknown>) {
		inputValues = values;
	}

	async function handleAddFolder() {
		try {
			const res = await pickFolder();
			if (res.path) {
				if (allowedPathsText.trim()) {
					allowedPathsText = allowedPathsText.trimEnd() + '\n' + res.path;
				} else {
					allowedPathsText = res.path;
				}
			}
		} catch {
			// Native dialog not available (headless) — ignore
		}
	}

	function removeFolder(index: number) {
		const lines = allowedPathsText.split('\n').filter(p => p.trim());
		lines.splice(index, 1);
		allowedPathsText = lines.join('\n');
	}

	async function handleSave() {
		saving = true;
		try {
			// Save inputs
			if (JSON.stringify(inputValues) !== savedInputValues) {
				await updateRoleInputs(channelState.activeRoleId, inputValues);
				savedInputValues = JSON.stringify(inputValues);
			}

			// Save allowed paths
			if (allowedPathsText !== savedPathsText) {
				const paths = allowedPathsText.split('\n').map(p => p.trim()).filter(p => p.length > 0);
				await updateEntityConfig('role', channelState.activeRoleId, {
					allowedPaths: paths,
				});
				savedPathsText = allowedPathsText;
			}
		} finally {
			saving = false;
		}
	}

	onMount(() => {
		load();
		// Detect Tauri
		isTauri = !!(window as any).__TAURI__;
	});
</script>

<svelte:head>
	<title>Nebo - {channelState.activeRoleName || $t('agent.configure')} - {$t('agent.configure')}</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-3xl mx-auto px-6 py-6">
		{#if loading}
			<div class="flex items-center justify-center py-8">
				<div class="loading loading-spinner loading-md"></div>
			</div>
		{:else}
			<!-- Role Inputs -->
			{#if inputFields.length > 0}
				<section class="pb-6">
					<div class="flex items-center justify-between mb-3 min-h-8">
						<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentConfigure.inputs')}</h2>
						<button
							class="btn btn-sm btn-primary"
							class:opacity-50={saving}
							disabled={saving || !hasChanges}
							onclick={handleSave}
						>
							{saving ? $t('common.saving') : $t('common.save')}
						</button>
					</div>
					<p class="text-xs text-base-content/70 mb-4">{$t('agentConfigure.inputsDesc')}</p>
					<RoleInputForm
						fields={inputFields}
						bind:values={inputValues}
						onchange={handleInputChange}
					/>
				</section>
			{/if}

			<!-- Allowed Paths -->
			<section class="pb-6 {inputFields.length > 0 ? 'border-t border-base-content/10 pt-4' : ''}">
				<div class="flex items-center justify-between mb-3 min-h-8">
					<h2 class="text-xs text-base-content/80 uppercase tracking-wider font-semibold">{$t('agentConfigure.allowedDirs')}</h2>
					<button
						type="button"
						class="btn btn-sm btn-ghost text-primary gap-1.5"
						onclick={handleAddFolder}
					>
						<FolderOpen class="w-3.5 h-3.5" />
						{$t('agentConfigure.addFolder')}
					</button>
				</div>
				<p class="text-xs text-base-content/70 mb-4">{$t('agentConfigure.allowedDirsDesc')}</p>

				<!-- Folder list -->
				{#if folders.length > 0}
					<div class="flex flex-col gap-1.5 mb-3">
						{#each folders as folder, i}
							<div class="flex items-center gap-2 rounded-lg bg-base-content/5 px-3 py-2">
								<FolderOpen class="w-4 h-4 text-base-content/70 shrink-0" />
								<span class="text-sm font-mono flex-1 min-w-0 truncate">{folder}</span>
								<button
									type="button"
									class="btn btn-xs btn-ghost btn-square text-base-content/70 hover:text-error"
									onclick={() => removeFolder(i)}
								>
									<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
										<line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
									</svg>
								</button>
							</div>
						{/each}
					</div>
				{:else}
					<div class="rounded-lg border border-dashed border-base-content/15 px-4 py-6 text-center mb-3">
						<p class="text-sm text-base-content/70">{$t('agentConfigure.noRestrictions')}</p>
						<p class="text-xs text-base-content/70 mt-1">{$t('agentConfigure.addFolderHint')}</p>
					</div>
				{/if}

				<!-- Hidden textarea for manual editing / fallback -->
				<details class="text-xs">
					<summary class="text-base-content/70 cursor-pointer hover:text-base-content/70">{$t('agentConfigure.editPaths')}</summary>
					<textarea
						class="textarea textarea-bordered w-full text-sm font-mono mt-2"
						rows="3"
						placeholder={$t('agentConfigure.pathPlaceholder')}
						value={allowedPathsText}
						oninput={(e) => allowedPathsText = (e.target as HTMLTextAreaElement).value}
					></textarea>
				</details>
			</section>
		{/if}
		</div>
	</div>
</div>
