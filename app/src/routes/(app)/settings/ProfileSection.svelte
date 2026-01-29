<script lang="ts">
	import { goto } from '$app/navigation';
	import { validateName, validatePassword, validatePasswordConfirmation } from '$lib/utils/validation';
	import { auth, currentUser, authLoading, authError } from '$lib/stores';
	import * as api from '$lib/api/gobot';
	import { tick } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import { User, Key, Trash2, LogOut } from 'lucide-svelte';

	// Profile form state
	let name = $state($currentUser?.name || '');
	let nameError = $state('');
	let nameTouched = $state(false);
	let profileSuccess = $state(false);
	let nameInputEl: HTMLInputElement | undefined = $state();

	// Password form state
	let currentPassword = $state('');
	let newPassword = $state('');
	let confirmPassword = $state('');
	let passwordError = $state('');
	let passwordSuccess = $state(false);
	let isChangingPassword = $state(false);

	// Delete account state
	let showDeleteConfirm = $state(false);
	let deletePassword = $state('');
	let deleteError = $state('');
	let isDeleting = $state(false);

	// Sync name with current user when it changes
	$effect(() => {
		if ($currentUser) {
			name = $currentUser.name;
		}
	});

	// Validation
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

	async function handlePasswordSubmit(event: SubmitEvent) {
		event.preventDefault();
		passwordError = '';
		passwordSuccess = false;

		// Validate new password
		const passwordValidation = validatePassword(newPassword);
		if (!passwordValidation.isValid) {
			passwordError = passwordValidation.error || 'Invalid password';
			return;
		}

		// Validate passwords match
		const matchValidation = validatePasswordConfirmation(newPassword, confirmPassword);
		if (!matchValidation.isValid) {
			passwordError = matchValidation.error || 'Passwords do not match';
			return;
		}

		isChangingPassword = true;
		try {
			await api.changePassword({
				currentPassword,
				newPassword
			});
			passwordSuccess = true;
			currentPassword = '';
			newPassword = '';
			confirmPassword = '';
		} catch (err: any) {
			passwordError = err?.message || 'Failed to change password';
		} finally {
			isChangingPassword = false;
		}
	}

	async function handleDeleteAccount() {
		if (!deletePassword) {
			deleteError = 'Please enter your password';
			return;
		}

		isDeleting = true;
		deleteError = '';

		try {
			await api.deleteAccount({ password: deletePassword });
			auth.logout();
			goto('/');
		} catch (err: any) {
			deleteError = err?.message || 'Failed to delete account';
		} finally {
			isDeleting = false;
		}
	}

	function handleLogout() {
		auth.logout();
		goto('/auth/login');
	}

	function formatDate(dateString: string): string {
		try {
			return new Date(dateString).toLocaleDateString(undefined, {
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
		<!-- Profile Information -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
					<User class="w-5 h-5 text-primary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Profile Information</h2>
					<p class="text-sm text-base-content/60">Update your personal details</p>
				</div>
			</div>

			{#if $authError}
				<div class="mb-4">
					<Alert type="error" title="Error">{$authError}</Alert>
				</div>
			{/if}

			{#if profileSuccess}
				<div class="mb-4">
					<Alert type="success" title="Success">Your profile has been updated.</Alert>
				</div>
			{/if}

			<form onsubmit={handleProfileSubmit} class="space-y-4">
				<!-- Email (Read-only) -->
				<div>
					<label for="email" class="block text-sm font-medium text-base-content/70 mb-2">Email</label>
					<input
						id="email"
						type="email"
						value={$currentUser.email}
						class="input w-full bg-base-700 cursor-not-allowed opacity-60"
						readonly
						disabled
					/>
					<p class="mt-1 text-xs text-base-content/50">Email cannot be changed</p>
				</div>

				<!-- Name -->
				<div>
					<label for="name" class="block text-sm font-medium text-base-content/70 mb-2">
						Full Name <span class="text-error">*</span>
					</label>
					<input
						id="name"
						type="text"
						bind:value={name}
						bind:this={nameInputEl}
						oninput={handleNameInput}
						placeholder="Your name"
						class="input w-full {(!nameValidation.isValid && nameTouched) || nameError ? 'border-error bg-error/10' : ''}"
						disabled={$authLoading}
					/>
					{#if (!nameValidation.isValid && nameTouched) || nameError}
						<p class="mt-1 text-sm text-error">{nameError || nameValidation.error}</p>
					{/if}
				</div>

				<!-- Account Info -->
				<div class="pt-4 border-t border-base-content/20 text-sm text-base-content/60 space-y-1">
					<p>Account ID: <span class="text-base-content/70 font-mono">{$currentUser.id}</span></p>
					<p>Member since: <span class="text-base-content/70">{formatDate($currentUser.createdAt)}</span></p>
				</div>

				<div class="pt-2">
					<Button type="primary" htmlType="submit" disabled={$authLoading || !nameTouched}>
						{#if $authLoading}
							<Spinner size={16} />
							Saving...
						{:else}
							Save Changes
						{/if}
					</Button>
				</div>
			</form>
		</Card>

		<!-- Change Password -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
					<Key class="w-5 h-5 text-secondary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Change Password</h2>
					<p class="text-sm text-base-content/60">Update your account password</p>
				</div>
			</div>

			{#if passwordError}
				<div class="mb-4">
					<Alert type="error" title="Error">{passwordError}</Alert>
				</div>
			{/if}

			{#if passwordSuccess}
				<div class="mb-4">
					<Alert type="success" title="Success">Your password has been changed.</Alert>
				</div>
			{/if}

			<form onsubmit={handlePasswordSubmit} class="space-y-4">
				<div>
					<label for="currentPassword" class="block text-sm font-medium text-base-content/70 mb-2">
						Current Password
					</label>
					<input
						id="currentPassword"
						type="password"
						bind:value={currentPassword}
						class="input w-full"
						disabled={isChangingPassword}
						autocomplete="current-password"
					/>
				</div>

				<div>
					<label for="newPassword" class="block text-sm font-medium text-base-content/70 mb-2">
						New Password
					</label>
					<input
						id="newPassword"
						type="password"
						bind:value={newPassword}
						class="input w-full"
						disabled={isChangingPassword}
						autocomplete="new-password"
					/>
					<p class="mt-1 text-xs text-base-content/50">At least 8 characters</p>
				</div>

				<div>
					<label for="confirmPassword" class="block text-sm font-medium text-base-content/70 mb-2">
						Confirm New Password
					</label>
					<input
						id="confirmPassword"
						type="password"
						bind:value={confirmPassword}
						class="input w-full"
						disabled={isChangingPassword}
						autocomplete="new-password"
					/>
				</div>

				<div class="pt-2">
					<Button
						type="secondary"
						htmlType="submit"
						disabled={isChangingPassword || !currentPassword || !newPassword || !confirmPassword}
					>
						{#if isChangingPassword}
							<Spinner size={16} />
							Changing...
						{:else}
							Change Password
						{/if}
					</Button>
				</div>
			</form>
		</Card>

		<!-- Session -->
		<Card>
			<div class="flex items-center gap-3 mb-4">
				<div class="w-10 h-10 rounded-xl bg-tertiary/10 flex items-center justify-center">
					<LogOut class="w-5 h-5 text-tertiary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Session</h2>
					<p class="text-sm text-base-content/60">Sign out of your account</p>
				</div>
			</div>

			<Button type="secondary" onclick={handleLogout}>
				Sign Out
			</Button>
		</Card>

		<!-- Delete Account -->
		<Card>
			<div class="flex items-center gap-3 mb-4">
				<div class="w-10 h-10 rounded-xl bg-error/10 flex items-center justify-center">
					<Trash2 class="w-5 h-5 text-error" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Delete Account</h2>
					<p class="text-sm text-base-content/60">Permanently delete your account and all data</p>
				</div>
			</div>

			{#if !showDeleteConfirm}
				<Button type="danger" onclick={() => (showDeleteConfirm = true)}>
					Delete Account
				</Button>
			{:else}
				<div class="p-4 bg-error/10 border border-error/30 rounded-lg space-y-4">
					<p class="text-sm text-base-content/70">
						This action cannot be undone. All your data will be permanently deleted.
					</p>

					{#if deleteError}
						<Alert type="error" title="Error">{deleteError}</Alert>
					{/if}

					<div>
						<label for="deletePassword" class="block text-sm font-medium text-base-content/70 mb-2">
							Enter your password to confirm
						</label>
						<input
							id="deletePassword"
							type="password"
							bind:value={deletePassword}
							class="input w-full"
							placeholder="Your password"
							disabled={isDeleting}
						/>
					</div>

					<div class="flex gap-3">
						<Button
							type="danger"
							onclick={handleDeleteAccount}
							disabled={!deletePassword || isDeleting}
						>
							{#if isDeleting}
								<Spinner size={16} />
								Deleting...
							{:else}
								Permanently Delete
							{/if}
						</Button>
						<Button type="ghost" onclick={() => { showDeleteConfirm = false; deletePassword = ''; deleteError = ''; }}>
							Cancel
						</Button>
					</div>
				</div>
			{/if}
		</Card>
	{:else if $authLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading profile...</p>
			</div>
		</Card>
	{:else}
		<Card>
			<div class="flex flex-col items-center gap-4 py-8 text-center">
				<p class="text-base-content/60">Unable to load profile. Please try refreshing.</p>
				<Button type="secondary" onclick={() => window.location.reload()}>
					Refresh Page
				</Button>
			</div>
		</Card>
	{/if}
</div>
