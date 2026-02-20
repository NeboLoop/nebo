<script lang="ts">
	import { onMount } from 'svelte';
	import { Cpu, Eye, Code, Brain, Sparkles, Volume2, Tag, Plus, Trash2, Activity } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';

	let isLoading = $state(true);
	let error = $state('');
	let providers = $state<components.AuthProfile[]>([]);
	let models = $state<{ [key: string]: components.ModelInfo[] }>({});
	let availableCLIs = $state<components.CLIAvailability | null>(null);
	let janusStatus = $state<components.NeboLoopAccountStatusResponse | null>(null);

	// Routing form state
	let routingForm = $state({
		vision: '',
		audio: '',
		reasoning: '',
		code: '',
		general: ''
	});
	let backupForm = $state({
		vision: '',
		audio: '',
		reasoning: '',
		code: '',
		general: ''
	});
	let aliasesForm = $state<{ alias: string; modelId: string }[]>([]);
	let laneRoutingForm = $state({
		heartbeat: '',
		events: '',
		comm: '',
		subagent: ''
	});
	let isSaving = $state(false);

	// CLI provider info loaded from models.yaml via API
	let cliProviderInfo = $state<{ [key: string]: { id: string; name: string; command: string; installHint: string; models: string[] } }>({});

	const janusCoveredProviders = ['anthropic', 'openai', 'google', 'deepseek'];

	const providerOptions = [
		{ value: 'anthropic', label: 'Anthropic (Claude)' },
		{ value: 'openai', label: 'OpenAI (GPT)' },
		{ value: 'google', label: 'Google (Gemini)' },
		{ value: 'deepseek', label: 'DeepSeek' },
		{ value: 'ollama', label: 'Ollama (Local)' }
	];

	const routingModes = [
		{ key: 'general' as const, label: 'All Purpose', description: 'Chat, Q&A, everyday tasks', icon: Sparkles, color: 'text-primary' },
		{ key: 'reasoning' as const, label: 'Reasoning', description: 'Analysis, problem solving', icon: Brain, color: 'text-secondary' },
		{ key: 'code' as const, label: 'Advanced Tasks', description: 'Code, PDFs, documents', icon: Code, color: 'text-accent' },
		{ key: 'vision' as const, label: 'Vision', description: 'Images, screenshots', icon: Eye, color: 'text-info' },
		{ key: 'audio' as const, label: 'Audio', description: 'Voice, transcription', icon: Volume2, color: 'text-warning' }
	];

	const laneModes = [
		{ key: 'heartbeat' as const, label: 'Heartbeat', description: 'Proactive check-ins' },
		{ key: 'events' as const, label: 'Scheduled Tasks', description: 'Cron jobs, reminders' },
		{ key: 'comm' as const, label: 'Communication', description: 'Inter-agent messages' },
		{ key: 'subagent' as const, label: 'Sub-agents', description: 'Background workers' }
	];

	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = '';
		try {
			const [modelsRes, profilesRes, janusRes] = await Promise.all([
				api.listModels(),
				api.listAuthProfiles(),
				api.neboLoopAccountStatus().catch(() => null)
			]);

			models = modelsRes.models || {};
			providers = profilesRes.profiles || [];
			janusStatus = janusRes;
			availableCLIs = modelsRes.availableCLIs || null;

			// Populate task routing form
			const taskRouting = modelsRes.taskRouting;
			if (taskRouting) {
				routingForm = {
					vision: taskRouting.vision || '',
					audio: taskRouting.audio || '',
					reasoning: taskRouting.reasoning || '',
					code: taskRouting.code || '',
					general: taskRouting.general || ''
				};
				const fb = taskRouting.fallbacks || {};
				backupForm = {
					vision: fb['vision']?.[0] || '',
					audio: fb['audio']?.[0] || '',
					reasoning: fb['reasoning']?.[0] || '',
					code: fb['code']?.[0] || '',
					general: fb['general']?.[0] || ''
				};
			}

			// Populate aliases
			aliasesForm = (modelsRes.aliases || []).map((a) => ({ alias: a.alias, modelId: a.modelId }));

			// Populate lane routing
			const lr = modelsRes.laneRouting;
			if (lr) {
				laneRoutingForm = {
					heartbeat: lr['heartbeat'] || '',
					events: lr['events'] || '',
					comm: lr['comm'] || '',
					subagent: lr['subagent'] || ''
				};
			}

			// CLI provider info
			if (modelsRes.cliProviders) {
				const info: { [key: string]: { id: string; name: string; command: string; installHint: string; models: string[] } } = {};
				for (const cp of modelsRes.cliProviders) {
					info[cp.command] = {
						id: cp.id,
						name: cp.displayName,
						command: cp.command,
						installHint: cp.installHint,
						models: cp.models || []
					};
				}
				cliProviderInfo = info;
			}
		} catch (err: any) {
			error = err?.message || 'Failed to load routing data';
		} finally {
			isLoading = false;
		}
	}

	function getGroupedModelOptions(): { provider: string; label: string; models: { value: string; label: string }[] }[] {
		const groups: { provider: string; label: string; models: { value: string; label: string }[] }[] = [];

		const configuredProviders = new Set(providers.filter(p => p.isActive).map(p => p.provider));
		const janusConnected = janusStatus?.connected && janusStatus.janusProvider;
		const cliProviderIds = new Set(Object.values(cliProviderInfo).map(c => c.id));

		// Janus first
		if (models['janus']) {
			const activeModels = models['janus'].filter(m => m.isActive);
			if (activeModels.length > 0) {
				groups.push({
					provider: 'janus',
					label: 'Janus (NeboLoop)',
					models: activeModels.map(m => ({
						value: `janus/${m.id}`,
						label: m.displayName
					}))
				});
			}
		}

		// API provider models
		for (const [providerType, modelList] of Object.entries(models)) {
			if (providerType === 'janus') continue;
			if (cliProviderIds.has(providerType)) continue;

			const hasApiKey = configuredProviders.has(providerType);
			const coveredByJanus = janusConnected && janusCoveredProviders.includes(providerType);
			if (!hasApiKey && !coveredByJanus) continue;

			const activeModels = modelList.filter(m => m.isActive);
			if (activeModels.length === 0) continue;

			const provLabel = providerOptions.find(p => p.value === providerType)?.label || providerType;
			groups.push({
				provider: providerType,
				label: provLabel,
				models: activeModels.map(m => ({
					value: `${providerType}/${m.id}`,
					label: m.displayName
				}))
			});
		}

		// CLI provider models
		for (const cli of Object.values(cliProviderInfo)) {
			const isAvailable =
				(cli.command === 'claude' && availableCLIs?.claude) ||
				(cli.command === 'codex' && availableCLIs?.codex) ||
				(cli.command === 'gemini' && availableCLIs?.gemini);
			if (!isAvailable) continue;

			if (models[cli.id]?.length) continue;

			groups.push({
				provider: cli.id,
				label: cli.name,
				models: cli.models.map(modelId => ({
					value: `${cli.id}/${modelId}`,
					label: modelId
				}))
			});
		}

		return groups;
	}

	function getAllModelOptions(): { value: string; label: string }[] {
		return getGroupedModelOptions().flatMap(g => g.models.map(m => ({ value: m.value, label: `${m.label} (${g.label})` })));
	}

	async function saveAll() {
		isSaving = true;
		error = '';
		try {
			const fallbacks: { [key: string]: string[] } = {};
			for (const mode of routingModes) {
				const backup = backupForm[mode.key];
				if (backup) {
					fallbacks[mode.key] = [backup];
				}
			}

			const validAliases = aliasesForm.filter((a) => a.alias.trim() && a.modelId);

			const laneRouting: { [key: string]: string } = {};
			if (laneRoutingForm.heartbeat) laneRouting['heartbeat'] = laneRoutingForm.heartbeat;
			if (laneRoutingForm.events) laneRouting['events'] = laneRoutingForm.events;
			if (laneRoutingForm.comm) laneRouting['comm'] = laneRoutingForm.comm;
			if (laneRoutingForm.subagent) laneRouting['subagent'] = laneRoutingForm.subagent;

			await api.updateTaskRouting({
				vision: routingForm.vision,
				audio: routingForm.audio,
				reasoning: routingForm.reasoning,
				code: routingForm.code,
				general: routingForm.general,
				fallbacks,
				aliases: validAliases,
				laneRouting: Object.keys(laneRouting).length > 0 ? laneRouting : undefined
			});
			await loadData();
		} catch (err: any) {
			error = err?.message || 'Failed to save routing';
		} finally {
			isSaving = false;
		}
	}

	function addAlias() {
		aliasesForm = [...aliasesForm, { alias: '', modelId: '' }];
	}

	function removeAlias(index: number) {
		aliasesForm = aliasesForm.filter((_, i) => i !== index);
	}
</script>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading routing configuration...</p>
			</div>
		</Card>
	{:else}
		{#if error}
			<Alert type="error" title="Error">{error}</Alert>
		{/if}

		<!-- Task Routing -->
		<Card>
			<div class="flex items-center justify-between mb-4">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
						<Cpu class="w-5 h-5 text-secondary" />
					</div>
					<div>
						<h3 class="text-lg font-semibold text-base-content">Task Routing</h3>
						<p class="text-sm text-base-content/60">Which model handles each type of task</p>
					</div>
				</div>
				<Button type="primary" size="sm" onclick={saveAll} disabled={isSaving}>
					{#if isSaving}
						<Spinner size={16} />
						Saving...
					{:else}
						Save
					{/if}
				</Button>
			</div>

			{@const groups = getGroupedModelOptions()}
			<div class="overflow-x-auto">
				<table class="table w-full">
					<thead>
						<tr>
							<th class="text-xs text-base-content/50 font-medium w-40">Task Type</th>
							<th class="text-xs text-base-content/50 font-medium">Main Model</th>
							<th class="text-xs text-base-content/50 font-medium">Backup</th>
							<th class="w-8"></th>
						</tr>
					</thead>
					<tbody>
						{#each routingModes as mode}
							<tr>
								<td>
									<div class="flex items-center gap-2">
										<mode.icon class="w-4 h-4 {mode.color} shrink-0" />
										<div>
											<span class="text-sm font-medium text-base-content">{mode.label}</span>
											<p class="text-xs text-base-content/40">{mode.description}</p>
										</div>
									</div>
								</td>
								<td>
									<select bind:value={routingForm[mode.key]} class="select select-bordered select-sm w-full">
										<option value="">Auto</option>
										{#each groups as group}
											<optgroup label={group.label}>
												{#each group.models as opt}
													<option value={opt.value}>{opt.label}</option>
												{/each}
											</optgroup>
										{/each}
									</select>
								</td>
								<td>
									<select bind:value={backupForm[mode.key]} class="select select-bordered select-sm w-full">
										<option value="">None</option>
										{#each groups as group}
											<optgroup label={group.label}>
												{#each group.models as opt}
													<option value={opt.value}>{opt.label}</option>
												{/each}
											</optgroup>
										{/each}
									</select>
								</td>
								<td></td>
							</tr>
						{/each}

						<!-- Separator for aliases -->
						{#if aliasesForm.filter(a => !['claude', 'codex', 'gemini'].includes(a.alias)).length > 0}
							<tr>
								<td colspan="4">
									<div class="flex items-center gap-2 pt-1">
										<Tag class="w-3.5 h-3.5 text-base-content/30" />
										<span class="text-xs text-base-content/40 font-medium">Custom Aliases</span>
									</div>
								</td>
							</tr>
						{/if}

						<!-- Custom alias rows -->
						{#each aliasesForm as aliasEntry, index}
							{#if !['claude', 'codex', 'gemini'].includes(aliasEntry.alias)}
								<tr>
									<td>
										<input type="text" placeholder="e.g. fast" bind:value={aliasEntry.alias} class="input input-bordered input-sm w-full" />
									</td>
									<td>
										<select bind:value={aliasEntry.modelId} class="select select-bordered select-sm w-full">
											<option value="">Select model...</option>
											{#each groups as group}
												<optgroup label={group.label}>
													{#each group.models as opt}
														<option value={opt.value}>{opt.label}</option>
													{/each}
												</optgroup>
											{/each}
										</select>
									</td>
									<td></td>
									<td>
										<button type="button" class="btn btn-ghost btn-sm btn-square" onclick={() => removeAlias(index)}>
											<Trash2 class="w-3.5 h-3.5 text-base-content/40" />
										</button>
									</td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
			</div>
			<div class="mt-2">
				<Button type="ghost" size="sm" onclick={addAlias}>
					<Plus class="w-4 h-4" /> Add Shortcut
				</Button>
			</div>
		</Card>

		<!-- Lane Routing -->
		<Card>
			<div class="flex items-center gap-3 mb-4">
				<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
					<Activity class="w-5 h-5 text-accent" />
				</div>
				<div>
					<h3 class="text-lg font-semibold text-base-content">Lane Routing</h3>
					<p class="text-sm text-base-content/60">Assign cheaper models to background lanes to reduce costs</p>
				</div>
			</div>

			{@const groups = getGroupedModelOptions()}
			<div class="overflow-x-auto">
				<table class="table w-full">
					<thead>
						<tr>
							<th class="text-xs text-base-content/50 font-medium w-40">Lane</th>
							<th class="text-xs text-base-content/50 font-medium">Model</th>
						</tr>
					</thead>
					<tbody>
						{#each laneModes as lane}
							<tr>
								<td>
									<div>
										<span class="text-sm font-medium text-base-content">{lane.label}</span>
										<p class="text-xs text-base-content/40">{lane.description}</p>
									</div>
								</td>
								<td>
									<select bind:value={laneRoutingForm[lane.key]} class="select select-bordered select-sm w-full">
										<option value="">Same as All Purpose</option>
										{#each groups as group}
											<optgroup label={group.label}>
												{#each group.models as opt}
													<option value={opt.value}>{opt.label}</option>
												{/each}
											</optgroup>
										{/each}
									</select>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</Card>
	{/if}
</div>
