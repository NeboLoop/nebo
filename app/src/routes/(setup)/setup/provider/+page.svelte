<script lang="ts">
	import { goto } from '$app/navigation';
	import { createAuthProfile, testAuthProfile } from '$lib/api';
	import { setup } from '$lib/stores/setup.svelte';
	import { StepCard, StepNavigation } from '$lib/components/setup';
	import { CheckCircle, XCircle } from 'lucide-svelte';

	const providerOptions = [
		{ value: 'anthropic', label: 'Anthropic (Claude)', defaultName: 'My Anthropic Key' },
		{ value: 'openai', label: 'OpenAI (GPT)', defaultName: 'My OpenAI Key' },
		{ value: 'google', label: 'Google (Gemini)', defaultName: 'My Google Key' },
		{ value: 'ollama', label: 'Ollama (Local)', defaultName: 'My Ollama Instance' }
	];

	let provider = $state('anthropic');
	let apiKey = $state('');
	let baseUrl = $state('');
	let name = $state('My Anthropic Key');
	let loading = $state(false);
	let error = $state('');
	let testLoading = $state(false);
	let testResult = $state<{ success: boolean; message: string } | null>(null);
	let createdProfileId = $state<string | null>(null);

	// Update default name when provider changes
	let previousProvider = $state('anthropic');
	$effect(() => {
		if (provider !== previousProvider) {
			const option = providerOptions.find((p) => p.value === provider);
			if (option) {
				name = option.defaultName;
			}
			previousProvider = provider;
			// Reset test result when provider changes
			testResult = null;
			createdProfileId = null;
		}
	});

	// Derived: whether API key is required
	let apiKeyRequired = $derived(provider !== 'ollama');

	// Derived: whether base URL field should show
	let showBaseUrl = $derived(provider === 'ollama');

	// Derived: form is valid for submission
	let formValid = $derived(
		name.trim() !== '' && (provider === 'ollama' || apiKey.trim() !== '')
	);

	async function handleTestConnection() {
		if (!formValid) {
			error = 'Please fill in required fields first';
			return;
		}

		testLoading = true;
		testResult = null;
		error = '';

		try {
			// First create the profile if not already created
			if (!createdProfileId) {
				const createResponse = await createAuthProfile({
					name: name.trim(),
					provider,
					apiKey: apiKey.trim(),
					baseUrl: baseUrl.trim() || undefined
				});
				createdProfileId = createResponse.profile.id;
			}

			// Now test it
			const response = await testAuthProfile({}, createdProfileId);
			testResult = { success: response.success, message: response.message };
		} catch (e: unknown) {
			testResult = {
				success: false,
				message: e instanceof Error ? e.message : 'Test failed'
			};
		} finally {
			testLoading = false;
		}
	}

	async function handleSubmit() {
		if (!formValid) {
			error = 'Please fill in required fields';
			return;
		}

		loading = true;
		error = '';

		try {
			// Create the profile if not already created during test
			if (!createdProfileId) {
				const createResponse = await createAuthProfile({
					name: name.trim(),
					provider,
					apiKey: apiKey.trim(),
					baseUrl: baseUrl.trim() || undefined
				});
				createdProfileId = createResponse.profile.id;
			}

			// Mark provider as configured
			setup.markProviderConfigured();

			// Navigate based on mode
			if (setup.state.mode === 'quickstart') {
				goto('/setup/complete');
			} else {
				goto('/setup/models');
			}
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to save provider';
		} finally {
			loading = false;
		}
	}

	function handleBack() {
		goto('/setup/account');
	}
</script>

<svelte:head>
	<title>Configure Provider - Nebo Setup</title>
</svelte:head>

<StepCard
	title="Configure AI Provider"
	description="Add an AI provider to power your Nebo. You can add more providers later in settings."
>
	{#if error}
		<div class="alert alert-error mb-4">
			<span>{error}</span>
		</div>
	{/if}

	<form onsubmit={(e) => { e.preventDefault(); handleSubmit(); }}>
		<div class="form-control mb-4">
			<label class="label" for="provider-type">
				<span class="label-text">Provider Type</span>
			</label>
			<select
				id="provider-type"
				bind:value={provider}
				class="select select-bordered w-full"
			>
				{#each providerOptions as opt}
					<option value={opt.value}>{opt.label}</option>
				{/each}
			</select>
		</div>

		<div class="form-control mb-4">
			<label class="label" for="provider-name">
				<span class="label-text">Name</span>
			</label>
			<input
				type="text"
				id="provider-name"
				bind:value={name}
				class="input input-bordered w-full"
				placeholder="My API Key"
				required
			/>
			<label class="label">
				<span class="label-text-alt text-base-content/60">A friendly name to identify this provider</span>
			</label>
		</div>

		<div class="form-control mb-4">
			<label class="label" for="api-key">
				<span class="label-text">
					API Key
					{#if !apiKeyRequired}
						<span class="text-base-content/60">(optional)</span>
					{/if}
				</span>
			</label>
			<input
				type="password"
				id="api-key"
				bind:value={apiKey}
				class="input input-bordered w-full"
				placeholder={provider === 'ollama' ? 'Not required for Ollama' : 'sk-...'}
				required={apiKeyRequired}
			/>
		</div>

		{#if showBaseUrl}
			<div class="form-control mb-4">
				<label class="label" for="base-url">
					<span class="label-text">Base URL</span>
				</label>
				<input
					type="text"
					id="base-url"
					bind:value={baseUrl}
					class="input input-bordered w-full"
					placeholder="http://localhost:11434"
				/>
				<label class="label">
					<span class="label-text-alt text-base-content/60">Leave empty for default (http://localhost:11434)</span>
				</label>
			</div>
		{/if}

		<!-- Test Connection -->
		<div class="mb-6">
			<button
				type="button"
				class="btn btn-outline btn-sm"
				onclick={handleTestConnection}
				disabled={testLoading || !formValid}
			>
				{#if testLoading}
					<span class="loading loading-spinner loading-sm"></span>
					Testing...
				{:else}
					Test Connection
				{/if}
			</button>

			{#if testResult}
				<div class="flex items-center gap-2 mt-2">
					{#if testResult.success}
						<CheckCircle class="w-5 h-5 text-success" />
						<span class="text-sm text-success">{testResult.message}</span>
					{:else}
						<XCircle class="w-5 h-5 text-error" />
						<span class="text-sm text-error">{testResult.message}</span>
					{/if}
				</div>
			{/if}
		</div>

		<StepNavigation
			showBack={true}
			onback={handleBack}
			onnext={handleSubmit}
			nextLabel="Continue"
			loading={loading}
		/>
	</form>
</StepCard>
