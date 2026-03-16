<!--
  Entity Config Panel — per-entity config (heartbeat, permissions, resources, model, personality).
  Shows inherited vs overridden state for each field.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { X, RotateCcw } from 'lucide-svelte';
	import { getEntityConfig, updateEntityConfig, type ResolvedEntityConfig } from '$lib/api/nebo';
	import Toggle from '$lib/components/ui/Toggle.svelte';

	interface Props {
		entityType: string;
		entityId: string;
		onclose?: () => void;
	}

	let { entityType, entityId, onclose }: Props = $props();

	let config = $state<ResolvedEntityConfig | null>(null);
	let loading = $state(true);
	let saving = $state(false);
	let error = $state('');

	const intervalOptions = [
		{ value: 5, label: '5 min' },
		{ value: 10, label: '10 min' },
		{ value: 15, label: '15 min' },
		{ value: 30, label: '30 min' },
		{ value: 60, label: '1 hr' },
		{ value: 120, label: '2 hr' },
		{ value: 240, label: '4 hr' },
		{ value: 480, label: '8 hr' },
		{ value: 1440, label: '24 hr' }
	];

	const permissionCategories = [
		{ key: 'web', label: 'Web Search' },
		{ key: 'desktop', label: 'Desktop Control' },
		{ key: 'filesystem', label: 'File System' },
		{ key: 'shell', label: 'Shell Commands' },
		{ key: 'memory', label: 'Memory Access' },
		{ key: 'calendar', label: 'Calendar' },
		{ key: 'email', label: 'Email' }
	];

	const resourceKinds = [
		{ key: 'screen', label: 'Screen Access' },
		{ key: 'browser', label: 'Browser Access' }
	];

	async function load() {
		loading = true;
		error = '';
		try {
			const res = await getEntityConfig(entityType, entityId);
			config = res.config;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load config';
		} finally {
			loading = false;
		}
	}

	async function save(patch: Record<string, unknown>) {
		saving = true;
		error = '';
		try {
			const res = await updateEntityConfig(entityType, entityId, patch);
			config = res.config;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to save';
		} finally {
			saving = false;
		}
	}

	async function clearField(field: string) {
		await save({ [field]: null });
	}

	onMount(load);
</script>

<div class="entity-config-panel">
	<div class="entity-config-panel-header">
		<span class="entity-config-panel-title">Entity Settings</span>
		<button class="btn btn-xs btn-ghost" onclick={onclose} title="Close">
			<X class="w-3.5 h-3.5" />
		</button>
	</div>

	{#if loading}
		<div class="entity-config-panel-loading">
			<span class="loading loading-spinner loading-sm"></span>
		</div>
	{:else if error}
		<div class="entity-config-panel-error">
			<p class="text-error text-base">{error}</p>
			<button class="btn btn-xs btn-ghost" onclick={load}>Retry</button>
		</div>
	{:else if config}
		<div class="entity-config-panel-body">
			<!-- Heartbeat -->
			<div class="entity-config-section">
				<div class="entity-config-section-header">
					<span class="entity-config-section-title">Heartbeat</span>
					{#if config.overrides['heartbeatEnabled']}
						<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => clearField('heartbeatEnabled')}>
							<RotateCcw class="w-3 h-3" />
						</button>
					{/if}
				</div>
				<Toggle
					checked={config.heartbeatEnabled}
					label="Enabled"
					size="sm"
					onchange={(v) => save({ heartbeatEnabled: v })}
				/>
				{#if config.heartbeatEnabled}
					<div class="entity-config-field-row">
						<label class="entity-config-label">Interval</label>
						<div class="entity-config-field-inline">
							<select
								class="select select-sm select-bordered"
								value={String(config.heartbeatIntervalMinutes)}
								onchange={(e) => save({ heartbeatIntervalMinutes: Number(e.currentTarget.value) })}
							>
								{#each intervalOptions as opt}
									<option value={String(opt.value)}>{opt.label}</option>
								{/each}
							</select>
							{#if config.overrides['heartbeatIntervalMinutes']}
								<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => clearField('heartbeatIntervalMinutes')}>
									<RotateCcw class="w-3 h-3" />
								</button>
							{/if}
						</div>
					</div>
					<div class="entity-config-field-row">
						<label class="entity-config-label">Time Window</label>
						<div class="entity-config-field-inline">
							<input
								type="time"
								class="input input-sm input-bordered"
								value={config.heartbeatWindow?.[0] ?? ''}
								onchange={(e) => save({
									heartbeatWindowStart: e.currentTarget.value,
									heartbeatWindowEnd: config?.heartbeatWindow?.[1] ?? '23:59'
								})}
							/>
							<span class="text-sm text-base-content/80">to</span>
							<input
								type="time"
								class="input input-sm input-bordered"
								value={config.heartbeatWindow?.[1] ?? ''}
								onchange={(e) => save({
									heartbeatWindowStart: config?.heartbeatWindow?.[0] ?? '00:00',
									heartbeatWindowEnd: e.currentTarget.value
								})}
							/>
							{#if config.overrides['heartbeatWindow']}
								<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => { clearField('heartbeatWindowStart'); clearField('heartbeatWindowEnd'); }}>
									<RotateCcw class="w-3 h-3" />
								</button>
							{/if}
						</div>
					</div>
					<div class="entity-config-field-row">
						<label class="entity-config-label">
							Content
							{#if config.overrides['heartbeatContent']}
								<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => clearField('heartbeatContent')}>
									<RotateCcw class="w-3 h-3" />
								</button>
							{/if}
						</label>
						<textarea
							class="textarea textarea-bordered textarea-sm entity-config-textarea"
							rows="3"
							placeholder="Inherited from HEARTBEAT.md"
							value={config.overrides['heartbeatContent'] ? config.heartbeatContent : ''}
							onblur={(e) => {
								const val = e.currentTarget.value.trim();
								if (val) save({ heartbeatContent: val });
								else clearField('heartbeatContent');
							}}
						></textarea>
					</div>
				{/if}
			</div>

			<!-- Permissions -->
			<div class="entity-config-section">
				<div class="entity-config-section-header">
					<span class="entity-config-section-title">Permissions</span>
					{#if config.overrides['permissions']}
						<button class="entity-config-reset-btn" title="Reset all to inherited" onclick={() => clearField('permissions')}>
							<RotateCcw class="w-3 h-3" />
						</button>
					{/if}
				</div>
				{#each permissionCategories as cat}
					<div class="entity-config-perm-row">
						<span class="entity-config-perm-label">{cat.label}</span>
						<div class="entity-config-perm-controls">
							<select
								class="select select-xs select-bordered"
								value={config.permissions[cat.key] === undefined ? 'inherit' : config.permissions[cat.key] ? 'allow' : 'deny'}
								onchange={(e) => {
									const val = e.currentTarget.value;
									if (val === 'inherit') {
										// Remove this key from permissions
										const perms = { ...config!.permissions };
										delete perms[cat.key];
										save({ permissions: Object.keys(perms).length ? perms : null });
									} else {
										const perms = { ...config!.permissions, [cat.key]: val === 'allow' };
										save({ permissions: perms });
									}
								}}
							>
								<option value="inherit">Inherit</option>
								<option value="allow">Allow</option>
								<option value="deny">Deny</option>
							</select>
						</div>
					</div>
				{/each}
			</div>

			<!-- Resource Access -->
			<div class="entity-config-section">
				<div class="entity-config-section-header">
					<span class="entity-config-section-title">Resource Access</span>
					{#if config.overrides['resourceGrants']}
						<button class="entity-config-reset-btn" title="Reset all to inherited" onclick={() => clearField('resourceGrants')}>
							<RotateCcw class="w-3 h-3" />
						</button>
					{/if}
				</div>
				{#each resourceKinds as res}
					<div class="entity-config-perm-row">
						<span class="entity-config-perm-label">{res.label}</span>
						<select
							class="select select-xs select-bordered"
							value={config.resourceGrants[res.key] ?? 'inherit'}
							onchange={(e) => {
								const grants = { ...config!.resourceGrants, [res.key]: e.currentTarget.value };
								save({ resourceGrants: grants });
							}}
						>
							<option value="inherit">Inherit</option>
							<option value="allow">Allow</option>
							<option value="deny">Deny</option>
						</select>
					</div>
				{/each}
			</div>

			<!-- Model Preference -->
			<div class="entity-config-section">
				<div class="entity-config-section-header">
					<span class="entity-config-section-title">Model</span>
					{#if config.overrides['modelPreference']}
						<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => clearField('modelPreference')}>
							<RotateCcw class="w-3 h-3" />
						</button>
					{/if}
				</div>
				<input
					type="text"
					class="input input-sm input-bordered"
					placeholder="Inherit from global routing"
					value={config.modelPreference ?? ''}
					onblur={(e) => {
						const val = e.currentTarget.value.trim();
						if (val) save({ modelPreference: val });
						else clearField('modelPreference');
					}}
				/>
			</div>

			<!-- Personality Snippet -->
			<div class="entity-config-section">
				<div class="entity-config-section-header">
					<span class="entity-config-section-title">Personality</span>
					{#if config.overrides['personalitySnippet']}
						<button class="entity-config-reset-btn" title="Reset to inherited" onclick={() => clearField('personalitySnippet')}>
							<RotateCcw class="w-3 h-3" />
						</button>
					{/if}
				</div>
				<textarea
					class="textarea textarea-bordered textarea-sm entity-config-textarea"
					rows="3"
					placeholder="Additional personality instructions for this entity"
					value={config.personalitySnippet ?? ''}
					onblur={(e) => {
						const val = e.currentTarget.value.trim();
						if (val) save({ personalitySnippet: val });
						else clearField('personalitySnippet');
					}}
				></textarea>
			</div>
		</div>
	{/if}
</div>
