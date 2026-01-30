<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { listModels, updateModel } from '$lib/api';
	import type { ModelInfo } from '$lib/api/gobotComponents';
	import { setup } from '$lib/stores/setup.svelte';
	import { StepCard, StepNavigation } from '$lib/components/setup';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import { Cpu } from 'lucide-svelte';

	let isLoading = $state(true);
	let models = $state<{ [key: string]: ModelInfo[] }>({});
	let error = $state('');
	let togglingModel = $state<string | null>(null);

	const providerLabels: { [key: string]: string } = {
		anthropic: 'Anthropic (Claude)',
		openai: 'OpenAI (GPT)',
		google: 'Google (Gemini)',
		ollama: 'Ollama (Local)'
	};

	onMount(async () => {
		await loadModels();
	});

	async function loadModels() {
		isLoading = true;
		error = '';
		try {
			const response = await listModels();
			models = response.models || {};
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load models';
		} finally {
			isLoading = false;
		}
	}

	async function handleToggleModel(providerType: string, model: ModelInfo) {
		const modelKey = `${providerType}-${model.id}`;
		togglingModel = modelKey;
		try {
			await updateModel({}, { active: !model.isActive }, providerType, model.id);
			await loadModels();
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to update model';
		} finally {
			togglingModel = null;
		}
	}

	function getProviderLabel(providerType: string): string {
		return providerLabels[providerType] || providerType;
	}

	function formatContextWindow(contextWindow: number | undefined): string {
		if (!contextWindow) return 'Unknown';
		if (contextWindow >= 1000000) {
			return `${(contextWindow / 1000000).toFixed(1)}M tokens`;
		}
		return `${(contextWindow / 1000).toFixed(0)}K tokens`;
	}

	// Check if any models exist
	let hasModels = $derived(Object.keys(models).some((key) => models[key].length > 0));

	function handleBack() {
		goto('/setup/provider');
	}

	function handleSkip() {
		goto('/setup/permissions');
	}

	function handleContinue() {
		goto('/setup/permissions');
	}
</script>

<svelte:head>
	<title>Configure Models - GoBot Setup</title>
</svelte:head>

<StepCard
	title="Configure Models"
	description="Choose which AI models GoBot can use. Toggle models on or off based on your preferences."
>
	{#if error}
		<div class="alert alert-error mb-4">
			<span>{error}</span>
		</div>
	{/if}

	{#if isLoading}
		<div class="flex flex-col items-center justify-center gap-4 py-8">
			<span class="loading loading-spinner loading-lg text-primary"></span>
			<p class="text-sm text-base-content/60">Loading available models...</p>
		</div>
	{:else if !hasModels}
		<div class="text-center py-8">
			<Cpu class="w-12 h-12 text-base-content/30 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-base-content mb-2">No models available</h3>
			<p class="text-sm text-base-content/60 mb-4">
				Configure a provider first to see available models.
			</p>
		</div>
	{:else}
		<div class="space-y-6">
			{#each Object.entries(models) as [providerType, providerModels] (providerType)}
				{#if providerModels.length > 0}
					<div class="border border-base-300 rounded-lg overflow-hidden">
						<div class="bg-base-200 px-4 py-3">
							<h3 class="font-medium text-base-content">{getProviderLabel(providerType)}</h3>
						</div>
						<div class="divide-y divide-base-200">
							{#each providerModels as model (model.id)}
								{@const modelKey = `${providerType}-${model.id}`}
								{@const isToggling = togglingModel === modelKey}
								<div class="flex items-center justify-between px-4 py-3">
									<div class="flex-1 min-w-0 pr-4">
										<p class="font-medium text-sm text-base-content truncate">{model.displayName}</p>
										<p class="text-xs text-base-content/50">
											{formatContextWindow(model.contextWindow)}
										</p>
									</div>
									<div class="flex items-center gap-2">
										{#if isToggling}
											<span class="loading loading-spinner loading-sm text-primary"></span>
										{/if}
										<Toggle
											checked={model.isActive}
											onchange={() => handleToggleModel(providerType, model)}
											disabled={isToggling}
											size="sm"
										/>
									</div>
								</div>
							{/each}
						</div>
					</div>
				{/if}
			{/each}
		</div>

		<!-- Info Card -->
		<div class="bg-base-200 rounded-lg p-4 mt-6">
			<h4 class="font-medium text-base-content mb-2">How it works</h4>
			<ul class="text-sm text-base-content/70 space-y-1 list-disc list-inside">
				<li>Toggle models on/off to control what GoBot can use</li>
				<li>GoBot automatically picks the best model for each task</li>
				<li>If one model fails, GoBot falls back to the next available</li>
			</ul>
		</div>
	{/if}

	<StepNavigation
		showBack={true}
		showSkip={true}
		onback={handleBack}
		onskip={handleSkip}
		onnext={handleContinue}
		nextLabel="Continue"
		class="mt-6"
	/>
</StepCard>
