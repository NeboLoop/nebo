<script lang="ts">
	import { onMount } from 'svelte';
	import { Cpu, Eye, Code, Brain, Sparkles, Volume2, Tag, Plus, Trash2, Activity } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let error = $state('');
	let providers = $state<components.AuthProfile[]>([]);
	let models = $state<{ [key: string]: components.ModelInfo[] }>({});
	let availableCLIs = $state<components.CLIAvailability | null>(null);
	let janusStatus = $state<components.NeboLoopAccountStatusResponse | null>(null);

	// Routing form state
	let routingForm = $state({
		vision: 'auto',
		audio: 'auto',
		reasoning: 'auto',
		code: 'auto',
		general: 'auto'
	});
	let backupForm = $state({
		vision: 'none',
		audio: 'none',
		reasoning: 'none',
		code: 'none',
		general: 'none'
	});
	let aliasesForm = $state<{ alias: string; modelId: string }[]>([]);
	let laneRoutingForm = $state({
		heartbeat: 'auto',
		events: 'auto',
		comm: 'auto',
		subagent: 'auto'
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
				// Build set of valid model option values
				const validValues = new Set(
					getGroupedModelOptions().flatMap(g => g.models.map(m => m.value))
				);
				const norm = (v: string | undefined) => (v && validValues.has(v)) ? v : 'auto';
				const normB = (v: string | undefined) => (v && validValues.has(v)) ? v : 'none';

				routingForm = {
					vision: norm(taskRouting.vision),
					audio: norm(taskRouting.audio),
					reasoning: norm(taskRouting.reasoning),
					code: norm(taskRouting.code),
					general: norm(taskRouting.general)
				};
				const fb = taskRouting.fallbacks || {};
				backupForm = {
					vision: normB(fb['vision']?.[0]),
					audio: normB(fb['audio']?.[0]),
					reasoning: normB(fb['reasoning']?.[0]),
					code: normB(fb['code']?.[0]),
					general: normB(fb['general']?.[0])
				};
			}

			// Populate aliases
			aliasesForm = (modelsRes.aliases || []).map((a) => ({ alias: a.alias, modelId: a.modelId }));

			// Populate lane routing
			const lr = modelsRes.laneRouting;
			if (lr) {
				laneRoutingForm = {
					heartbeat: lr['heartbeat'] || 'auto',
					events: lr['events'] || 'auto',
					comm: lr['comm'] || 'auto',
					subagent: lr['subagent'] || 'auto'
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
			const toApi = (v: string) => (v === 'auto' || v === 'none') ? '' : v;
			const fallbacks: { [key: string]: string[] } = {};
			for (const mode of routingModes) {
				const backup = toApi(backupForm[mode.key]);
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
				vision: toApi(routingForm.vision),
				audio: toApi(routingForm.audio),
				reasoning: toApi(routingForm.reasoning),
				code: toApi(routingForm.code),
				general: toApi(routingForm.general),
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

	const groups = $derived(getGroupedModelOptions());
	const allModelValues = $derived(new Set(groups.flatMap(g => g.models.map(m => m.value))));

	function formatModelId(id: string): string {
		// "anthropic/claude-sonnet-4-5-20250929" → "claude-sonnet-4-5-20250929"
		const parts = id.split('/');
		return parts.length > 1 ? parts.slice(1).join('/') : id;
	}

	function addAlias() {
		aliasesForm = [...aliasesForm, { alias: '', modelId: '' }];
	}

	function removeAlias(index: number) {
		aliasesForm = aliasesForm.filter((_, i) => i !== index);
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Routing</h2>
	<p class="text-base text-base-content/80">Which model handles each type of task</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading routing configuration...</span>
	</div>
{:else}
	<div class="space-y-6">
		{#if error}
			<div class="rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-base text-error">
				{error}
			</div>
		{/if}

		<!-- Task Routing -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Task Routing</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="space-y-5">
					{#each routingModes as mode}
						<div>
							<div class="flex items-center gap-2 mb-2">
								<mode.icon class="w-4 h-4 {mode.color} shrink-0" />
								<span class="text-base font-medium text-base-content">{mode.label}</span>
								<span class="text-base text-base-content/80">{mode.description}</span>
							</div>
							<div class="grid sm:grid-cols-2 gap-3">
								<div>
									<label class="text-base font-medium text-base-content/80">Main model</label>
									<select bind:value={routingForm[mode.key]} class="w-full h-11 mt-1 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors">
										<option value="auto">Auto</option>
										{#each groups as group}
											<optgroup label={group.label}>
												{#each group.models as opt}
													<option value={opt.value}>{opt.label}</option>
												{/each}
											</optgroup>
										{/each}
									</select>
								</div>
								<div>
									<label class="text-base font-medium text-base-content/80">Backup</label>
									<select bind:value={backupForm[mode.key]} class="w-full h-11 mt-1 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors">
										<option value="none">None</option>
										{#each groups as group}
											<optgroup label={group.label}>
												{#each group.models as opt}
													<option value={opt.value}>{opt.label}</option>
												{/each}
											</optgroup>
										{/each}
									</select>
								</div>
							</div>
						</div>
					{/each}
				</div>

				<!-- Custom Aliases -->
				{#if aliasesForm.filter(a => !['claude', 'codex', 'gemini'].includes(a.alias)).length > 0}
					<div class="mt-5 pt-5 border-t border-base-content/10">
						<div class="flex items-center gap-2 mb-3">
							<Tag class="w-3.5 h-3.5 text-base-content/90" />
							<span class="text-base font-medium text-base-content/80">Custom Aliases</span>
						</div>
						<div class="space-y-3">
							{#each aliasesForm as aliasEntry, index}
								{#if !['claude', 'codex', 'gemini'].includes(aliasEntry.alias)}
									<div class="flex items-center gap-3">
										<input
											type="text"
											placeholder="e.g. fast"
											bind:value={aliasEntry.alias}
											class="w-40 h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
										/>
										<select bind:value={aliasEntry.modelId} class="flex-1 h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors">
											<option value="">Select model...</option>
											{#each groups as group}
												<optgroup label={group.label}>
													{#each group.models as opt}
														<option value={opt.value}>{opt.label}</option>
													{/each}
												</optgroup>
											{/each}
										</select>
										<button
											type="button"
											class="w-11 h-11 rounded-xl bg-base-content/5 border border-base-content/10 flex items-center justify-center hover:border-base-content/40 transition-colors"
											onclick={() => removeAlias(index)}
											aria-label="Remove alias"
										>
											<Trash2 class="w-4 h-4 text-base-content/90" />
										</button>
									</div>
								{/if}
							{/each}
						</div>
					</div>
				{/if}

				<div class="mt-4">
					<button
						type="button"
						class="flex items-center gap-2 text-base font-medium text-base-content/80 hover:text-primary transition-colors"
						onclick={addAlias}
					>
						<Plus class="w-4 h-4" /> Add shortcut
					</button>
				</div>
			</div>
		</section>

		<!-- Lane Routing -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-1">Lane Routing</h3>
			<p class="text-base text-base-content/80 mb-3">Assign cheaper models to background lanes to reduce costs</p>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="space-y-5">
					{#each laneModes as lane}
						<div>
							<div class="flex items-center gap-2 mb-2">
								<span class="text-base font-medium text-base-content">{lane.label}</span>
								<span class="text-base text-base-content/80">{lane.description}</span>
							</div>
							<select bind:value={laneRoutingForm[lane.key]} class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors">
								<option value="auto">Auto</option>
								{#each groups as group}
									<optgroup label={group.label}>
										{#each group.models as opt}
											<option value={opt.value}>{opt.label}</option>
										{/each}
									</optgroup>
								{/each}
							</select>
						</div>
					{/each}
				</div>
			</div>
		</section>

		<!-- Save -->
		<div class="flex justify-end">
			<button
				type="button"
				disabled={isSaving}
				class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
				onclick={saveAll}
			>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Routing
				{/if}
			</button>
		</div>
	</div>
{/if}
