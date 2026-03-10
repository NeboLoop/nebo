<script lang="ts">
	import { validateEmail, validatePassword, validatePasswordConfirmation, type PasswordRequirements } from '$lib/utils/validation';
	import { passwordReset } from '$lib/stores';
	import { tick } from 'svelte';

	type Mode = 'request' | 'reset';

	interface Props {
		mode?: Mode;
		token?: string;
		onSuccess?: () => void;
		onBackToLogin?: () => void;
	}

	let { mode = 'request', token = '', onSuccess, onBackToLogin }: Props = $props();

	// Request mode state
	let email = $state('');
	let emailError = $state('');
	let emailTouched = $state(false);

	// Reset mode state
	let newPassword = $state('');
	let confirmPassword = $state('');
	let passwordError = $state('');
	let confirmError = $state('');
	let showPassword = $state(false);
	let passwordTouched = $state(false);
	let confirmTouched = $state(false);

	let emailInputEl: HTMLInputElement | undefined = $state();
	let passwordInputEl: HTMLInputElement | undefined = $state();
	let confirmInputEl: HTMLInputElement | undefined = $state();

	// Get store values
	let isLoading = $derived($passwordReset.isLoading);
	let isSuccess = $derived($passwordReset.isSuccess);
	let storeError = $derived($passwordReset.error);

	// Generate unique IDs for accessibility
	const formId = $derived(`password-reset-form-${Math.random().toString(36).substr(2, 9)}`);
	const emailId = $derived(`${formId}-email`);
	const passwordId = $derived(`${formId}-password`);
	const confirmId = $derived(`${formId}-confirm`);
	const emailErrorId = $derived(`${formId}-email-error`);
	const passwordErrorId = $derived(`${formId}-password-error`);
	const confirmErrorId = $derived(`${formId}-confirm-error`);
	const generalErrorId = $derived(`${formId}-general-error`);
	const passwordHintId = $derived(`${formId}-password-hint`);

	// Reactive validations
	const emailValidation = $derived.by(() => {
		if (!emailTouched && !email) return { isValid: true };
		return validateEmail(email);
	});

	const passwordValidation = $derived.by(() => {
		if (!passwordTouched && !newPassword) return { isValid: true, requirements: undefined as PasswordRequirements | undefined };
		return validatePassword(newPassword);
	});

	const confirmValidation = $derived.by(() => {
		if (!confirmTouched && !confirmPassword) return { isValid: true };
		return validatePasswordConfirmation(newPassword, confirmPassword);
	});

	const hasEmailError = $derived((!emailValidation.isValid && emailTouched) || !!emailError);
	const hasPasswordError = $derived((!passwordValidation.isValid && passwordTouched) || !!passwordError);
	const hasConfirmError = $derived((!confirmValidation.isValid && confirmTouched) || !!confirmError);

	function handleEmailInput() {
		emailTouched = true;
		emailError = '';
		passwordReset.reset();
	}

	function handlePasswordInput() {
		passwordTouched = true;
		passwordError = '';
		passwordReset.reset();
		// Re-validate confirm if already touched
		if (confirmTouched && confirmPassword) {
			const result = validatePasswordConfirmation(newPassword, confirmPassword);
			confirmError = result.isValid ? '' : (result.error || '');
		}
	}

	function handleConfirmInput() {
		confirmTouched = true;
		confirmError = '';
		passwordReset.reset();
	}

	function togglePasswordVisibility() {
		showPassword = !showPassword;
	}

	async function handleRequestSubmit(event: SubmitEvent) {
		event.preventDefault();
		emailTouched = true;

		const emailResult = validateEmail(email);
		if (!emailResult.isValid) {
			emailError = emailResult.error || 'Invalid email';
			await tick();
			emailInputEl?.focus();
			return;
		}

		emailError = '';
		const success = await passwordReset.requestReset(email.trim());

		if (success) {
			onSuccess?.();
		}
	}

	async function handleResetSubmit(event: SubmitEvent) {
		event.preventDefault();
		passwordTouched = true;
		confirmTouched = true;

		const passwordResult = validatePassword(newPassword);
		if (!passwordResult.isValid) {
			passwordError = passwordResult.error || 'Invalid password';
			await tick();
			passwordInputEl?.focus();
			return;
		}

		const confirmResult = validatePasswordConfirmation(newPassword, confirmPassword);
		if (!confirmResult.isValid) {
			confirmError = confirmResult.error || 'Passwords do not match';
			await tick();
			confirmInputEl?.focus();
			return;
		}

		passwordError = '';
		confirmError = '';

		const success = await passwordReset.resetPassword(token, newPassword);

		if (success) {
			onSuccess?.();
		}
	}
</script>

{#if mode === 'request'}
	<!-- Request Password Reset Form -->
	{#if isSuccess}
		<div class="card bg-base-200 w-full max-w-md mx-auto" role="status" aria-live="polite">
			<div class="card-body text-center">
				<div class="flex justify-center mb-4">
					<div class="w-16 h-16 rounded-full bg-success/20 flex items-center justify-center">
						<svg class="w-8 h-8 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
							<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path>
							<polyline points="22 4 12 14.01 9 11.01"></polyline>
						</svg>
					</div>
				</div>
				<h2 class="card-title justify-center text-xl">Check Your Email</h2>
				<p class="text-base-content/60 text-sm">
					We've sent password reset instructions to <strong>{email}</strong>.
					Please check your inbox and follow the link to reset your password.
				</p>
				<p class="text-base-content/40 text-xs mt-2">
					Didn't receive the email? Check your spam folder or
					<button type="button" class="link link-primary" onclick={() => passwordReset.reset()}>
						try again
					</button>
				</p>
				{#if onBackToLogin}
					<div class="mt-4">
						<button type="button" class="btn btn-ghost w-full" onclick={onBackToLogin}>
							Back to Sign In
						</button>
					</div>
				{/if}
			</div>
		</div>
	{:else}
		<form class="card bg-base-200 w-full max-w-md mx-auto" onsubmit={handleRequestSubmit} aria-label="Password reset request form">
			<div class="card-body">
				<div class="text-center mb-4">
					<h2 class="card-title justify-center text-2xl">Reset Password</h2>
					<p class="text-base-content/60 text-sm">
						Enter your email address and we'll send you instructions to reset your password.
					</p>
				</div>

				{#if storeError}
					<div id={generalErrorId} class="alert alert-error mb-4" role="alert" aria-live="assertive">
						<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
						</svg>
						<span>{storeError}</span>
					</div>
				{/if}

				<div class="space-y-4">
					<div class="form-control w-full">
						<label for={emailId} class="label">
							<span class="label-text">Email Address <span class="text-error">*</span></span>
						</label>
						<input
							id={emailId}
							type="email"
							bind:value={email}
							bind:this={emailInputEl}
							oninput={handleEmailInput}
							placeholder="you@example.com"
							class="input input-bordered w-full"
							class:input-error={hasEmailError}
							disabled={isLoading}
							aria-describedby={hasEmailError ? emailErrorId : undefined}
							aria-invalid={hasEmailError}
							aria-required="true"
							autocomplete="email"
						/>
						{#if hasEmailError}
							<label class="label" id={emailErrorId}>
								<span class="label-text-alt text-error">{emailError || emailValidation.error}</span>
							</label>
						{/if}
					</div>
				</div>

				<div class="form-control mt-6">
					<button
						type="submit"
						class="btn btn-primary w-full"
						disabled={isLoading}
						aria-busy={isLoading}
					>
						{#if isLoading}
							<span class="loading loading-spinner loading-sm"></span>
							Sending...
						{:else}
							Send Reset Link
						{/if}
					</button>
				</div>

				{#if onBackToLogin}
					<div class="divider"></div>
					<p class="text-center text-sm text-base-content/60">
						Remember your password?
						<button
							type="button"
							class="link link-primary"
							onclick={onBackToLogin}
							disabled={isLoading}
						>
							Sign in
						</button>
					</p>
				{/if}
			</div>
		</form>
	{/if}

{:else}
	<!-- Reset Password Form -->
	{#if isSuccess}
		<div class="card bg-base-200 w-full max-w-md mx-auto" role="status" aria-live="polite">
			<div class="card-body text-center">
				<div class="flex justify-center mb-4">
					<div class="w-16 h-16 rounded-full bg-success/20 flex items-center justify-center">
						<svg class="w-8 h-8 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
							<path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"></path>
							<polyline points="22 4 12 14.01 9 11.01"></polyline>
						</svg>
					</div>
				</div>
				<h2 class="card-title justify-center text-xl">Password Reset Complete</h2>
				<p class="text-base-content/60 text-sm">
					Your password has been successfully reset. You can now sign in with your new password.
				</p>
				{#if onBackToLogin}
					<div class="mt-4">
						<button type="button" class="btn btn-primary w-full" onclick={onBackToLogin}>
							Sign In
						</button>
					</div>
				{/if}
			</div>
		</div>
	{:else}
		<form class="card bg-base-200 w-full max-w-md mx-auto" onsubmit={handleResetSubmit} aria-label="Create new password form">
			<div class="card-body">
				<div class="text-center mb-4">
					<h2 class="card-title justify-center text-2xl">Create New Password</h2>
					<p class="text-base-content/60 text-sm">Enter your new password below.</p>
				</div>

				{#if storeError}
					<div id={generalErrorId} class="alert alert-error mb-4" role="alert" aria-live="assertive">
						<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
						</svg>
						<span>{storeError}</span>
					</div>
				{/if}

				<div class="space-y-4">
					<!-- New Password Field -->
					<div class="form-control w-full">
						<label for={passwordId} class="label">
							<span class="label-text">New Password <span class="text-error">*</span></span>
						</label>
						<div class="relative">
							<input
								id={passwordId}
								type={showPassword ? 'text' : 'password'}
								bind:value={newPassword}
								bind:this={passwordInputEl}
								oninput={handlePasswordInput}
								placeholder="Enter your new password"
								class="input input-bordered w-full pr-12"
								class:input-error={hasPasswordError}
								disabled={isLoading}
								aria-describedby={[passwordHintId, hasPasswordError ? passwordErrorId : ''].filter(Boolean).join(' ')}
								aria-invalid={hasPasswordError}
								aria-required="true"
								autocomplete="new-password"
							/>
							<button
								type="button"
								class="btn btn-ghost btn-sm absolute right-1 top-1/2 -translate-y-1/2"
								onclick={togglePasswordVisibility}
								aria-label={showPassword ? 'Hide password' : 'Show password'}
								disabled={isLoading}
							>
								{#if showPassword}
									<svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
										<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"></path>
										<line x1="1" y1="1" x2="23" y2="23"></line>
									</svg>
								{:else}
									<svg class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
										<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
										<circle cx="12" cy="12" r="3"></circle>
									</svg>
								{/if}
							</button>
						</div>
						{#if hasPasswordError}
							<label class="label" id={passwordErrorId}>
								<span class="label-text-alt text-error">{passwordError || passwordValidation.error}</span>
							</label>
						{/if}

						<!-- Password Requirements -->
						{#if passwordTouched && passwordValidation.requirements}
							<div id={passwordHintId} class="mt-2 p-3 bg-base-300 rounded-lg" aria-label="Password requirements">
								<ul class="text-xs space-y-1">
									<li class="flex items-center gap-2" class:text-success={passwordValidation.requirements.minLength}>
										<span>{passwordValidation.requirements.minLength ? '✓' : '○'}</span>
										At least 8 characters
									</li>
									<li class="flex items-center gap-2" class:text-success={passwordValidation.requirements.hasUppercase}>
										<span>{passwordValidation.requirements.hasUppercase ? '✓' : '○'}</span>
										One uppercase letter
									</li>
									<li class="flex items-center gap-2" class:text-success={passwordValidation.requirements.hasLowercase}>
										<span>{passwordValidation.requirements.hasLowercase ? '✓' : '○'}</span>
										One lowercase letter
									</li>
									<li class="flex items-center gap-2" class:text-success={passwordValidation.requirements.hasNumber}>
										<span>{passwordValidation.requirements.hasNumber ? '✓' : '○'}</span>
										One number
									</li>
								</ul>
							</div>
						{:else}
							<p class="text-xs text-base-content/60 mt-1" id={passwordHintId}>
								Password must be at least 8 characters with uppercase, lowercase, and numbers
							</p>
						{/if}
					</div>

					<!-- Confirm Password Field -->
					<div class="form-control w-full">
						<label for={confirmId} class="label">
							<span class="label-text">Confirm New Password <span class="text-error">*</span></span>
						</label>
						<input
							id={confirmId}
							type={showPassword ? 'text' : 'password'}
							bind:value={confirmPassword}
							bind:this={confirmInputEl}
							oninput={handleConfirmInput}
							placeholder="Confirm your new password"
							class="input input-bordered w-full"
							class:input-error={hasConfirmError}
							disabled={isLoading}
							aria-describedby={hasConfirmError ? confirmErrorId : undefined}
							aria-invalid={hasConfirmError}
							aria-required="true"
							autocomplete="new-password"
						/>
						{#if hasConfirmError}
							<label class="label" id={confirmErrorId}>
								<span class="label-text-alt text-error">{confirmError || confirmValidation.error}</span>
							</label>
						{/if}
					</div>
				</div>

				<div class="form-control mt-6">
					<button
						type="submit"
						class="btn btn-primary w-full"
						disabled={isLoading}
						aria-busy={isLoading}
					>
						{#if isLoading}
							<span class="loading loading-spinner loading-sm"></span>
							Resetting...
						{:else}
							Reset Password
						{/if}
					</button>
				</div>

				{#if onBackToLogin}
					<div class="divider"></div>
					<p class="text-center text-sm">
						<button
							type="button"
							class="link link-primary"
							onclick={onBackToLogin}
							disabled={isLoading}
						>
							Back to Sign In
						</button>
					</p>
				{/if}
			</div>
		</form>
	{/if}
{/if}
