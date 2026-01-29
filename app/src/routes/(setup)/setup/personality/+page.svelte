<script lang="ts">
	import { goto } from '$app/navigation';
	import { getPersonality, updatePersonality } from '$lib/api';
	import { StepCard, StepNavigation } from '$lib/components/setup';

	let content = $state('');
	let originalContent = $state('');
	let loading = $state(false);
	let saving = $state(false);
	let error = $state('');

	// Fetch personality content on mount
	$effect(() => {
		fetchPersonality();
	});

	async function fetchPersonality() {
		loading = true;
		error = '';
		try {
			const response = await getPersonality();
			content = response.content;
			originalContent = response.content;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load personality';
		} finally {
			loading = false;
		}
	}

	async function handleSave(): Promise<boolean> {
		saving = true;
		error = '';
		try {
			await updatePersonality({ content });
			originalContent = content;
			return true;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to save personality';
			return false;
		} finally {
			saving = false;
		}
	}

	async function handleContinue() {
		// Save if content has changed
		if (content !== originalContent) {
			const saved = await handleSave();
			if (!saved) return;
		}
		goto('/setup/complete');
	}

	function handleSkip() {
		goto('/setup/complete');
	}

	function handleBack() {
		goto('/setup/permissions');
	}

	function handleReset() {
		content = originalContent;
	}

	// Derived: whether content has been modified
	let hasChanges = $derived(content !== originalContent);
</script>

<svelte:head>
	<title>Personality - GoBot Setup</title>
</svelte:head>

<StepCard
	title="Customize Personality"
	description="Define how your GoBot behaves and communicates. This is the SOUL.md content that shapes your agent's personality."
>
	{#if error}
		<div class="alert alert-error mb-4">
			<span>{error}</span>
		</div>
	{/if}

	{#if loading}
		<div class="flex justify-center py-8">
			<span class="loading loading-spinner loading-lg"></span>
		</div>
	{:else}
		<div class="form-control mb-4">
			<label class="label" for="personality-content">
				<span class="label-text">Personality Content</span>
				{#if hasChanges}
					<span class="label-text-alt text-warning">Unsaved changes</span>
				{/if}
			</label>
			<textarea
				id="personality-content"
				bind:value={content}
				class="textarea textarea-bordered w-full min-h-64 font-mono text-sm"
				placeholder="Enter personality instructions..."
			></textarea>
			<label class="label">
				<span class="label-text-alt text-base-content/60">
					This content defines how your AI agent communicates and behaves.
				</span>
			</label>
		</div>

		<div class="flex justify-end mb-6">
			<button
				type="button"
				class="btn btn-outline btn-sm"
				onclick={handleReset}
				disabled={!hasChanges || saving}
			>
				Reset Changes
			</button>
		</div>
	{/if}

	<StepNavigation
		showBack={true}
		showSkip={true}
		onback={handleBack}
		onskip={handleSkip}
		onnext={handleContinue}
		nextLabel="Continue"
		loading={saving}
	/>
</StepCard>
