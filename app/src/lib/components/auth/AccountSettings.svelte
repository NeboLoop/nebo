<script lang="ts">
	import { validateName } from '$lib/utils/validation';
	import { auth, currentUser, authLoading, authError } from '$lib/stores';
	import { tick } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';

	interface Props {
		onLogout?: () => void;
	}

	let { onLogout }: Props = $props();

	// Profile form state
	let name = $state($currentUser?.name || '');
	let nameError = $state('');
	let nameTouched = $state(false);
	let profileSuccess = $state(false);

	let nameInputEl: HTMLInputElement | undefined = $state();

	// Sync name with current user when it changes
	$effect(() => {
		if ($currentUser) {
			name = $currentUser.name;
		}
	});

	// Generate unique IDs for accessibility
	const formId = $derived(`account-form-${Math.random().toString(36).substr(2, 9)}`);
	const nameId = $derived(`${formId}-name`);
	const emailId = $derived(`${formId}-email`);
	const nameErrorId = $derived(`${formId}-name-error`);

	// Reactive validation
	const nameValidation = $derived.by(() => {
		if (!nameTouched && !name) return { isValid: true };
		return validateName(name);
	});

	function handleNameInput() {
		nameTouched = true;
		nameError = '';
		profileSuccess = false;
		auth.clearError();
	}

	async function handleProfileSubmit(event: SubmitEvent) {
		event.preventDefault();
		nameTouched = true;
		profileSuccess = false;

		const nameResult = validateName(name);
		if (!nameResult.isValid) {
			nameError = nameResult.error || 'Invalid name';
			await tick();
			nameInputEl?.focus();
			return;
		}

		nameError = '';

		const success = await auth.updateProfile({ name: name.trim() });

		if (success) {
			profileSuccess = true;
			nameTouched = false;
		}
	}

	function handleLogout() {
		auth.logout();
		onLogout?.();
	}

	// Format date
	function formatDate(dateString: string): string {
		try {
			const date = new Date(dateString);
			return date.toLocaleDateString(undefined, {
				year: 'numeric',
				month: 'long',
				day: 'numeric'
			});
		} catch {
			return dateString;
		}
	}
</script>

<div class="space-y-6">
	{#if $currentUser}
		<!-- Profile Section -->
		<Card>
			<section aria-labelledby="profile-heading">
				<h3 id="profile-heading" class="text-lg font-semibold text-white mb-6">Profile Information</h3>

				{#if $authError}
					<div class="mb-4">
						<Alert type="error" title="Error">{$authError}</Alert>
					</div>
				{/if}

				{#if profileSuccess}
					<div class="mb-4">
						<Alert type="success" title="Success">Your profile has been updated successfully.</Alert>
					</div>
				{/if}

				<form onsubmit={handleProfileSubmit} aria-label="Update profile form" class="space-y-4">
					<!-- Email (Read-only) -->
					<div>
						<label for={emailId} class="block text-sm font-medium text-base-300 mb-2">Email Address</label>
						<input
							id={emailId}
							type="email"
							value={$currentUser.email}
							class="input w-full bg-base-700 cursor-not-allowed opacity-60"
							readonly
							disabled
							aria-describedby="email-note"
						/>
						<p id="email-note" class="mt-1 text-xs text-base-500">
							Email address cannot be changed
						</p>
					</div>

					<!-- Name -->
					<div>
						<label for={nameId} class="block text-sm font-medium text-base-300 mb-2">
							Full Name
							<span class="text-error" aria-hidden="true">*</span>
						</label>
						<input
							id={nameId}
							type="text"
							bind:value={name}
							bind:this={nameInputEl}
							oninput={handleNameInput}
							placeholder="Your name"
							class="input w-full {(!nameValidation.isValid && nameTouched) || nameError ? 'border-error bg-error/10' : ''}"
							disabled={$authLoading}
							aria-describedby={(!nameValidation.isValid && nameTouched) || nameError ? nameErrorId : undefined}
							aria-invalid={(!nameValidation.isValid && nameTouched) || !!nameError}
							aria-required="true"
							autocomplete="name"
						/>
						{#if (!nameValidation.isValid && nameTouched) || nameError}
							<p id={nameErrorId} class="mt-1 text-sm text-error" role="alert">
								{nameError || nameValidation.error}
							</p>
						{/if}
					</div>

					<div class="pt-2">
						<Button
							type="primary"
							htmlType="submit"
							disabled={$authLoading || !nameTouched}
						>
							{#if $authLoading}
								<Spinner size={16} />
								Saving...
							{:else}
								Save Changes
							{/if}
						</Button>
					</div>
				</form>
			</section>
		</Card>

		<!-- Account Details Section -->
		<Card>
			<section aria-labelledby="account-details-heading">
				<h3 id="account-details-heading" class="text-lg font-semibold text-white mb-6">Account Details</h3>

				<dl class="space-y-4">
					<div class="flex justify-between items-center py-2 border-b border-base-700">
						<dt class="text-sm text-base-400">Account ID</dt>
						<dd class="text-sm text-base-300 font-mono">{$currentUser.id}</dd>
					</div>
					<div class="flex justify-between items-center py-2 border-b border-base-700">
						<dt class="text-sm text-base-400">Member Since</dt>
						<dd class="text-sm text-base-300">{formatDate($currentUser.createdAt)}</dd>
					</div>
					{#if $currentUser.updatedAt !== $currentUser.createdAt}
						<div class="flex justify-between items-center py-2">
							<dt class="text-sm text-base-400">Last Updated</dt>
							<dd class="text-sm text-base-300">{formatDate($currentUser.updatedAt)}</dd>
						</div>
					{/if}
				</dl>
			</section>
		</Card>

		<!-- Session Section -->
		<Card>
			<section aria-labelledby="session-heading">
				<h3 id="session-heading" class="text-lg font-semibold text-white mb-2">Session</h3>
				<p class="text-sm text-base-400 mb-4">
					Sign out of your account on this device.
				</p>

				<Button
					type="secondary"
					onclick={handleLogout}
					disabled={$authLoading}
				>
					Sign Out
				</Button>
			</section>
		</Card>
	{:else if $authLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-400">Loading account...</p>
			</div>
		</Card>
	{:else}
		<Card>
			<div class="flex flex-col items-center gap-4 py-8 text-center">
				<p class="text-base-400">Unable to load account information. Please try refreshing the page.</p>
				<Button type="secondary" onclick={() => window.location.reload()}>
					Refresh Page
				</Button>
			</div>
		</Card>
	{/if}
</div>
