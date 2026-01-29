<script lang="ts">
	import { onMount } from 'svelte';
	import { validateEmail, validatePassword, validatePasswordConfirmation, validateName, type PasswordRequirements } from '$lib/utils/validation';
	import { auth, authError, authLoading } from '$lib/stores';
	import * as api from '$lib/api/gobot';
	import { tick } from 'svelte';

	interface Props {
		onSuccess?: () => void;
		onLoginClick?: () => void;
	}

	let { onSuccess, onLoginClick }: Props = $props();

	let name = $state('');
	let email = $state('');
	let password = $state('');
	let confirmPassword = $state('');
	let showPassword = $state(false);

	let nameError = $state('');
	let emailError = $state('');
	let passwordError = $state('');
	let confirmError = $state('');

	let touched = $state({ name: false, email: false, password: false, confirm: false });

	let nameInputEl: HTMLInputElement | undefined = $state();
	let emailInputEl: HTMLInputElement | undefined = $state();
	let passwordInputEl: HTMLInputElement | undefined = $state();
	let confirmInputEl: HTMLInputElement | undefined = $state();

	// OAuth state
	let googleEnabled = $state(false);
	let githubEnabled = $state(false);
	let oauthLoading = $state<string | null>(null);

	// Generate unique IDs for accessibility
	const formId = $derived(`register-form-${Math.random().toString(36).substr(2, 9)}`);
	const nameId = $derived(`${formId}-name`);
	const emailId = $derived(`${formId}-email`);
	const passwordId = $derived(`${formId}-password`);
	const confirmId = $derived(`${formId}-confirm`);
	const nameErrorId = $derived(`${formId}-name-error`);
	const emailErrorId = $derived(`${formId}-email-error`);
	const passwordErrorId = $derived(`${formId}-password-error`);
	const confirmErrorId = $derived(`${formId}-confirm-error`);
	const generalErrorId = $derived(`${formId}-general-error`);
	const passwordHintId = $derived(`${formId}-password-hint`);

	// Reactive validations
	const nameValidation = $derived.by(() => {
		if (!touched.name && !name) return { isValid: true };
		return validateName(name);
	});

	const emailValidation = $derived.by(() => {
		if (!touched.email && !email) return { isValid: true };
		return validateEmail(email);
	});

	const passwordValidation = $derived.by(() => {
		if (!touched.password && !password) return { isValid: true, requirements: undefined as PasswordRequirements | undefined };
		return validatePassword(password);
	});

	const confirmValidation = $derived.by(() => {
		if (!touched.confirm && !confirmPassword) return { isValid: true };
		return validatePasswordConfirmation(password, confirmPassword);
	});

	const hasNameError = $derived((!nameValidation.isValid && touched.name) || !!nameError);
	const hasEmailError = $derived((!emailValidation.isValid && touched.email) || !!emailError);
	const hasPasswordError = $derived((!passwordValidation.isValid && touched.password) || !!passwordError);
	const hasConfirmError = $derived((!confirmValidation.isValid && touched.confirm) || !!confirmError);
	const hasOAuth = $derived(googleEnabled || githubEnabled);

	onMount(async () => {
		// Fetch auth config to see which OAuth providers are enabled
		try {
			const config = await api.getAuthConfig();
			googleEnabled = config.googleEnabled;
			githubEnabled = config.githubEnabled;
		} catch {
			// OAuth not available, that's fine
		}
	});

	function handleNameInput() {
		touched.name = true;
		nameError = '';
		auth.clearError();
	}

	function handleEmailInput() {
		touched.email = true;
		emailError = '';
		auth.clearError();
	}

	function handlePasswordInput() {
		touched.password = true;
		passwordError = '';
		auth.clearError();
		// Re-validate confirm if already touched
		if (touched.confirm && confirmPassword) {
			const result = validatePasswordConfirmation(password, confirmPassword);
			confirmError = result.isValid ? '' : (result.error || '');
		}
	}

	function handleConfirmInput() {
		touched.confirm = true;
		confirmError = '';
		auth.clearError();
	}

	async function handleOAuthLogin(provider: 'google' | 'github') {
		oauthLoading = provider;
		auth.clearError();

		try {
			const response = await api.getOAuthUrl({}, provider);
			// Redirect to OAuth provider
			window.location.href = response.url;
		} catch (error) {
			console.error('OAuth error:', error);
			oauthLoading = null;
		}
	}

	function togglePasswordVisibility() {
		showPassword = !showPassword;
	}

	async function handleSubmit(event: SubmitEvent) {
		event.preventDefault();
		touched = { name: true, email: true, password: true, confirm: true };

		// Validate all fields
		const nameResult = validateName(name);
		if (!nameResult.isValid) {
			nameError = nameResult.error || 'Invalid name';
			await tick();
			nameInputEl?.focus();
			return;
		}

		const emailResult = validateEmail(email);
		if (!emailResult.isValid) {
			emailError = emailResult.error || 'Invalid email';
			await tick();
			emailInputEl?.focus();
			return;
		}

		const passwordResult = validatePassword(password);
		if (!passwordResult.isValid) {
			passwordError = passwordResult.error || 'Invalid password';
			await tick();
			passwordInputEl?.focus();
			return;
		}

		const confirmResult = validatePasswordConfirmation(password, confirmPassword);
		if (!confirmResult.isValid) {
			confirmError = confirmResult.error || 'Passwords do not match';
			await tick();
			confirmInputEl?.focus();
			return;
		}

		nameError = '';
		emailError = '';
		passwordError = '';
		confirmError = '';

		const result = await auth.register({
			name: name.trim(),
			email: email.trim(),
			password
		});

		if (result.success) {
			onSuccess?.();
		}
	}
</script>

<form class="card bg-base-200 w-full max-w-md mx-auto" onsubmit={handleSubmit} aria-label="Registration form">
	<div class="card-body">
		<div class="text-center mb-4">
			<h2 class="card-title justify-center text-2xl">Create Account</h2>
			<p class="text-base-content/60 text-sm">Create your account to get started</p>
		</div>

		{#if $authError}
			<div id={generalErrorId} class="alert alert-error mb-4" role="alert" aria-live="assertive">
				<svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
				</svg>
				<span>{$authError}</span>
			</div>
		{/if}

		{#if hasOAuth}
			<div class="space-y-3 mb-4">
				{#if googleEnabled}
					<button
						type="button"
						class="btn btn-outline w-full"
						onclick={() => handleOAuthLogin('google')}
						disabled={$authLoading || oauthLoading !== null}
					>
						{#if oauthLoading === 'google'}
							<span class="loading loading-spinner loading-sm"></span>
						{:else}
							<svg class="w-5 h-5" viewBox="0 0 24 24">
								<path fill="currentColor" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
								<path fill="currentColor" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/>
								<path fill="currentColor" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/>
								<path fill="currentColor" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/>
							</svg>
						{/if}
						Continue with Google
					</button>
				{/if}

				{#if githubEnabled}
					<button
						type="button"
						class="btn btn-outline w-full"
						onclick={() => handleOAuthLogin('github')}
						disabled={$authLoading || oauthLoading !== null}
					>
						{#if oauthLoading === 'github'}
							<span class="loading loading-spinner loading-sm"></span>
						{:else}
							<svg class="w-5 h-5" viewBox="0 0 24 24" fill="currentColor">
								<path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
							</svg>
						{/if}
						Continue with GitHub
					</button>
				{/if}

				<div class="divider text-xs text-base-content/50">or continue with email</div>
			</div>
		{/if}

		<div class="space-y-4">
			<!-- Name Field -->
			<div class="form-control w-full">
				<label for={nameId} class="label">
					<span class="label-text">Full Name <span class="text-error">*</span></span>
				</label>
				<input
					id={nameId}
					type="text"
					bind:value={name}
					bind:this={nameInputEl}
					oninput={handleNameInput}
					placeholder="John Doe"
					class="input input-bordered w-full"
					class:input-error={hasNameError}
					disabled={$authLoading || oauthLoading !== null}
					aria-describedby={hasNameError ? nameErrorId : undefined}
					aria-invalid={hasNameError}
					aria-required="true"
					autocomplete="name"
				/>
				{#if hasNameError}
					<label class="label" id={nameErrorId}>
						<span class="label-text-alt text-error">{nameError || nameValidation.error}</span>
					</label>
				{/if}
			</div>

			<!-- Email Field -->
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
					disabled={$authLoading || oauthLoading !== null}
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

			<!-- Password Field -->
			<div class="form-control w-full">
				<label for={passwordId} class="label">
					<span class="label-text">Password <span class="text-error">*</span></span>
				</label>
				<div class="relative">
					<input
						id={passwordId}
						type={showPassword ? 'text' : 'password'}
						bind:value={password}
						bind:this={passwordInputEl}
						oninput={handlePasswordInput}
						placeholder="Create a strong password"
						class="input input-bordered w-full pr-12"
						class:input-error={hasPasswordError}
						disabled={$authLoading || oauthLoading !== null}
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
						disabled={$authLoading || oauthLoading !== null}
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
				{#if touched.password && passwordValidation.requirements}
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
							<li class="flex items-center gap-2 italic" class:text-success={passwordValidation.requirements.hasSpecialChar}>
								<span>{passwordValidation.requirements.hasSpecialChar ? '✓' : '○'}</span>
								Special character (recommended)
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
					<span class="label-text">Confirm Password <span class="text-error">*</span></span>
				</label>
				<input
					id={confirmId}
					type={showPassword ? 'text' : 'password'}
					bind:value={confirmPassword}
					bind:this={confirmInputEl}
					oninput={handleConfirmInput}
					placeholder="Confirm your password"
					class="input input-bordered w-full"
					class:input-error={hasConfirmError}
					disabled={$authLoading || oauthLoading !== null}
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
				disabled={$authLoading || oauthLoading !== null}
				aria-busy={$authLoading}
			>
				{#if $authLoading}
					<span class="loading loading-spinner loading-sm"></span>
					Creating account...
				{:else}
					Create Account
				{/if}
			</button>
		</div>

		<p class="text-center text-xs text-base-content/60 mt-4">
			By creating an account, you agree to our
			<a href="/terms" class="link link-primary">Terms of Service</a>
			and
			<a href="/privacy" class="link link-primary">Privacy Policy</a>
		</p>

		{#if onLoginClick}
			<div class="divider"></div>
			<p class="text-center text-sm text-base-content/60">
				Already have an account?
				<button
					type="button"
					class="link link-primary"
					onclick={() => {
						auth.clearError();
						onLoginClick?.();
					}}
					disabled={$authLoading || oauthLoading !== null}
				>
					Sign in
				</button>
			</p>
		{/if}
	</div>
</form>
