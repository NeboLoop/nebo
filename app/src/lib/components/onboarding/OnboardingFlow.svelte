<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Key,
		Sparkles,
		ArrowRight,
		Check,
		Loader2,
		Terminal,
		Shield,
		FileText,
		Monitor,
		Globe,
		Users,
		Camera,
		Cpu,
		MessageCircle,
		Store,
		CircleCheck,
		ChevronDown
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import { neboLoopRegister, neboLoopLogin, neboLoopAccountStatus } from '$lib/api';
	import type * as components from '$lib/api/neboComponents';
	import Button from '$lib/components/ui/Button.svelte';

	type OnboardingStep = 'welcome' | 'terms' | 'provider-choice' | 'api-key' | 'capabilities' | 'neboloop' | 'complete';
	type ProviderChoice = 'janus' | 'claude-code' | 'codex-cli' | 'gemini-cli' | 'api-key';

	let currentStep = $state<OnboardingStep>('welcome');
	let error = $state('');
	let providerChoice = $state<ProviderChoice | null>(null);
	let cameFromJanus = $state(false);
	let showMoreProviders = $state(false);

	// CLI detection
	let isCheckingCLI = $state(true);
	let cliStatuses = $state<components.CLIStatusMap | null>(null);

	// API Key form
	let apiKey = $state('');
	let provider = $state<'anthropic' | 'openai' | 'google'>('anthropic');
	let isTestingKey = $state(false);
	let keyValid = $state(false);
	let isSettingUpCLI = $state(false);

	// Terms
	let termsAccepted = $state(false);
	let isAcceptingTerms = $state(false);

	// Capabilities
	let permissions = $state<Record<string, boolean>>({
		chat: true,
		file: false,
		shell: false,
		web: false,
		contacts: false,
		desktop: false,
		media: false,
		system: false
	});
	let isSavingPermissions = $state(false);

	// NeboLoop
	let neboLoopTab = $state<'signup' | 'login'>('signup');
	let neboLoopLoading = $state(false);
	let neboLoopError = $state('');
	let neboLoopConnected = $state(false);
	let neboLoopEmail = $state('');
	let signupEmail = $state('');
	let signupName = $state('');
	let signupPassword = $state('');
	let signupConfirm = $state('');
	let nlLoginEmail = $state('');
	let nlLoginPassword = $state('');

	const capabilityGroups = [
		{
			key: 'chat',
			label: 'Chat & Memory',
			description: 'Conversations, memory storage, scheduled tasks',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			label: 'File System',
			description: 'Read, write, and edit files on your computer',
			icon: FileText
		},
		{
			key: 'shell',
			label: 'Shell & Terminal',
			description: 'Run commands and manage processes',
			icon: Terminal
		},
		{
			key: 'web',
			label: 'Web Browsing',
			description: 'Fetch web pages, search the internet, browser automation',
			icon: Globe
		},
		{
			key: 'contacts',
			label: 'Contacts & Calendar',
			description: 'Access contacts, calendar, reminders, and mail',
			icon: Users
		},
		{
			key: 'desktop',
			label: 'Desktop Control',
			description: 'Window management, accessibility, clipboard',
			icon: Monitor
		},
		{
			key: 'media',
			label: 'Media & Capture',
			description: 'Screenshots, image analysis, music, text-to-speech',
			icon: Camera
		},
		{
			key: 'system',
			label: 'System',
			description: 'Spotlight search, keychain, shortcuts, system info',
			icon: Cpu
		}
	];

	const providerInfo = {
		anthropic: {
			name: 'Anthropic (Claude)',
			placeholder: 'sk-ant-...',
			helpUrl: 'https://console.anthropic.com/account/keys'
		},
		openai: {
			name: 'OpenAI (GPT)',
			placeholder: 'sk-...',
			helpUrl: 'https://platform.openai.com/api-keys'
		},
		google: {
			name: 'Google (Gemini)',
			placeholder: 'AI...',
			helpUrl: 'https://aistudio.google.com/apikey'
		}
	};

	const cliProviderInfo: Record<string, { id: string; name: string; description: string; model: string }> = {
		claude: {
			id: 'claude-code',
			name: 'Claude Agent',
			description: 'Use your existing Claude subscription',
			model: 'claude-code/opus'
		},
		codex: {
			id: 'codex-cli',
			name: 'Codex CLI',
			description: 'Use your ChatGPT/OpenAI subscription',
			model: 'codex-cli/gpt-5'
		},
		gemini: {
			id: 'gemini-cli',
			name: 'Gemini CLI',
			description: 'Use your Google AI subscription (FREE)',
			model: 'gemini-cli/gemini-2.5-pro'
		}
	};

	// Get list of authenticated CLIs
	let authenticatedCLIs = $derived(() => {
		if (!cliStatuses) return [];
		const result: string[] = [];
		if (cliStatuses.claude?.authenticated) result.push('claude');
		if (cliStatuses.codex?.authenticated) result.push('codex');
		if (cliStatuses.gemini?.authenticated) result.push('gemini');
		return result;
	});

	// Auto-select Janus as default
	$effect(() => {
		if (providerChoice === null) {
			providerChoice = 'janus';
		}
	});

	// Progress dots - steps visible to user
	const progressSteps = ['welcome', 'terms', 'provider-choice', 'capabilities', 'neboloop', 'complete'];

	onMount(async () => {
		// Check CLI statuses (installed + authenticated)
		try {
			const response = await api.listModels();
			cliStatuses = response.cliStatuses ?? null;
		} catch {
			cliStatuses = null;
		} finally {
			isCheckingCLI = false;
		}
	});

	async function acceptTerms() {
		isAcceptingTerms = true;
		error = '';
		try {
			await api.acceptTerms();
			currentStep = 'provider-choice';
		} catch (err: any) {
			error = err?.message || 'Failed to accept terms';
		} finally {
			isAcceptingTerms = false;
		}
	}

	async function setupCLI(cliKey: string) {
		isSettingUpCLI = true;
		error = '';

		try {
			const info = cliProviderInfo[cliKey];
			// Update models.yaml to use this CLI as primary
			await api.updateModelConfig({
				primary: info.model
			});

			currentStep = 'capabilities';
		} catch (err: any) {
			error = err?.message || `Failed to configure ${cliProviderInfo[cliKey].name}`;
		} finally {
			isSettingUpCLI = false;
		}
	}

	async function testAndSaveApiKey() {
		if (!apiKey.trim()) {
			error = 'Please enter an API key';
			return;
		}

		isTestingKey = true;
		error = '';
		keyValid = false;

		try {
			// Create the auth profile
			const profileResponse = await api.createAuthProfile({
				name: `My ${providerInfo[provider].name}`,
				provider: provider,
				apiKey: apiKey.trim()
			});

			// Test it
			const testResponse = await api.testAuthProfile(profileResponse.profile.id);

			if (testResponse.success) {
				keyValid = true;
				setTimeout(() => {
					currentStep = 'capabilities';
				}, 500);
			} else {
				error = testResponse.message || 'API key validation failed';
				// Delete the invalid profile
				await api.deleteAuthProfile(profileResponse.profile.id);
			}
		} catch (err: any) {
			error = err?.message || 'Failed to save API key';
		} finally {
			isTestingKey = false;
		}
	}

	async function savePermissionsAndFinish() {
		isSavingPermissions = true;
		error = '';

		try {
			await api.updateToolPermissions({ permissions });
			if (cameFromJanus) {
				// Already connected to NeboLoop via Janus path — skip to complete
				await completeOnboarding();
			} else {
				currentStep = 'neboloop';
				// Check NeboLoop status when entering the step
				checkNeboLoopStatus();
			}
		} catch (err: any) {
			error = err?.message || 'Failed to save permissions';
		} finally {
			isSavingPermissions = false;
		}
	}

	async function checkNeboLoopStatus() {
		try {
			const status = await neboLoopAccountStatus();
			if (status.connected) {
				neboLoopConnected = true;
				neboLoopEmail = status.email ?? '';
			}
		} catch {
			// Not connected, that's fine
		}
	}

	async function handleNeboLoopSignup() {
		neboLoopError = '';
		if (!signupEmail || !signupName || !signupPassword) {
			neboLoopError = 'All fields are required';
			return;
		}
		if (signupPassword !== signupConfirm) {
			neboLoopError = 'Passwords do not match';
			return;
		}
		if (signupPassword.length < 8) {
			neboLoopError = 'Password must be at least 8 characters';
			return;
		}

		neboLoopLoading = true;
		try {
			const resp = await neboLoopRegister({
				email: signupEmail,
				displayName: signupName,
				password: signupPassword
			});
			neboLoopConnected = true;
			neboLoopEmail = resp.email;
		} catch (e: any) {
			neboLoopError = e?.message || 'Registration failed. Please try again.';
		} finally {
			neboLoopLoading = false;
		}
	}

	async function handleNeboLoopLogin() {
		neboLoopError = '';
		if (!nlLoginEmail || !nlLoginPassword) {
			neboLoopError = 'Email and password are required';
			return;
		}

		neboLoopLoading = true;
		try {
			const resp = await neboLoopLogin({
				email: nlLoginEmail,
				password: nlLoginPassword
			});
			neboLoopConnected = true;
			neboLoopEmail = resp.email;
		} catch (e: any) {
			neboLoopError = e?.message || 'Login failed. Please check your credentials.';
		} finally {
			neboLoopLoading = false;
		}
	}

	async function completeOnboarding() {
		try {
			await api.updateUserProfile({ onboardingCompleted: true });
		} catch {
			// Non-fatal — still proceed to complete
		}
		currentStep = 'complete';
	}

	async function finishOnboarding() {
		// Force full page reload to re-check onboarding status in layout
		window.location.href = '/agent';
	}

	function handleProviderChoiceContinue() {
		if (!providerChoice) return;

		if (providerChoice === 'janus') {
			cameFromJanus = true;
			currentStep = 'neboloop';
			checkNeboLoopStatus();
		} else if (providerChoice === 'api-key') {
			currentStep = 'api-key';
		} else {
			// Map provider choice to CLI key
			const cliKey = getCLIKeyFromChoice(providerChoice);
			setupCLI(cliKey);
		}
	}

	function getCLIKeyFromChoice(choice: ProviderChoice): string {
		// Map provider IDs to CLI keys
		const mapping: Record<string, string> = {
			'claude-code': 'claude',
			'codex-cli': 'codex',
			'gemini-cli': 'gemini'
		};
		return mapping[choice] || choice.replace('-cli', '');
	}

	function togglePermission(key: string) {
		if (key === 'chat') return; // Chat is always on
		permissions = { ...permissions, [key]: !permissions[key] };
	}
</script>

<div class="fixed inset-0 bg-base-100 z-50 flex items-center justify-center">
	<div class="w-full max-w-lg p-8">
		<!-- Progress dots -->
		<div class="flex justify-center gap-2 mb-8">
			{#each progressSteps as step}
				{@const stepIndex = progressSteps.indexOf(currentStep === 'api-key' ? 'provider-choice' : currentStep)}
				{@const dotIndex = progressSteps.indexOf(step)}
				<div
					class="w-2 h-2 rounded-full transition-colors {stepIndex >= dotIndex
						? 'bg-primary'
						: 'bg-base-300'}"
				></div>
			{/each}
		</div>

		<!-- Welcome Step -->
		{#if currentStep === 'welcome'}
			<div class="text-center animate-in fade-in duration-300">
				<div class="w-20 h-20 rounded-full bg-primary/20 flex items-center justify-center mx-auto mb-6">
					<Sparkles class="w-10 h-10 text-primary" />
				</div>
				<h1 class="text-3xl font-bold mb-3">Welcome to Nebo</h1>
				<p class="text-base-content/70 mb-8 text-lg">
					Your personal AI assistant. Let's get you set up in just a minute.
				</p>
				<Button type="primary" size="lg" onclick={() => (currentStep = 'terms')}>
					Get Started
					<ArrowRight class="w-5 h-5 ml-2" />
				</Button>
			</div>
		{/if}

		<!-- Terms Step -->
		{#if currentStep === 'terms'}
			<div class="animate-in fade-in duration-300">
				<div class="w-16 h-16 rounded-full bg-warning/20 flex items-center justify-center mx-auto mb-6">
					<Shield class="w-8 h-8 text-warning" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">Privacy & Terms</h2>
				<p class="text-base-content/70 text-center mb-6">
					Important information about how Nebo handles your data
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				<div class="bg-base-200 rounded-xl p-5 mb-6 space-y-4 text-sm max-h-64 overflow-y-auto">
					<div>
						<h3 class="font-semibold text-base-content mb-1">Your Data Stays Local</h3>
						<p class="text-base-content/70">
							All conversations, memories, and settings are stored locally on your device in a SQLite database. Nothing is sent to Nebo's servers.
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">AI Provider Communication</h3>
						<p class="text-base-content/70">
							When you chat, your messages are sent to your chosen AI provider (Anthropic, OpenAI, or Google) for processing. Their privacy policies apply to that data.
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">API Keys</h3>
						<p class="text-base-content/70">
							Your API keys are stored locally in your device's database. They are only used to authenticate with your AI provider.
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">System Access</h3>
						<p class="text-base-content/70">
							Nebo can access system features (files, shell, contacts, etc.) but only capabilities you explicitly enable. You control what the agent can do.
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">No Analytics or Telemetry</h3>
						<p class="text-base-content/70">
							Nebo does not collect usage analytics, telemetry, or crash reports. Your usage is completely private.
						</p>
					</div>
				</div>

				<label class="flex items-start gap-3 mb-6 cursor-pointer">
					<input
						type="checkbox"
						class="checkbox checkbox-primary mt-0.5"
						bind:checked={termsAccepted}
					/>
					<span class="text-sm text-base-content/80">
						I understand that my conversations are sent to my chosen AI provider for processing, and that I control which system capabilities the agent can access.
					</span>
				</label>

				<Button
					type="primary"
					class="w-full"
					onclick={acceptTerms}
					disabled={!termsAccepted || isAcceptingTerms}
				>
					{#if isAcceptingTerms}
						<Loader2 class="w-5 h-5 mr-2 animate-spin" />
						Saving...
					{:else}
						Continue
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- Provider Choice Step -->
		{#if currentStep === 'provider-choice'}
			<div class="animate-in fade-in duration-300">
				<div class="w-16 h-16 rounded-full bg-primary/20 flex items-center justify-center mx-auto mb-6">
					<Sparkles class="w-8 h-8 text-primary" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">Connect Your AI</h2>
				<p class="text-base-content/70 text-center mb-6">
					Choose how to power Nebo
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				<div class="space-y-3 mb-6">
					<!-- Janus - Primary option -->
					<button
						type="button"
						class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice === 'janus'
							? 'border-primary bg-primary/5'
							: 'border-base-300 hover:border-base-content/30'}"
						onclick={() => (providerChoice = 'janus')}
					>
						<div class="flex items-start gap-3">
							<div class="p-2 rounded-lg bg-primary/20">
								<Sparkles class="w-5 h-5 text-primary" />
							</div>
							<div class="flex-1">
								<div class="flex items-center gap-2">
									<span class="font-semibold">Janus</span>
									<span class="badge badge-primary badge-sm">Recommended</span>
								</div>
								<p class="text-sm text-base-content/60 mt-1">
									AI powered by NeboLoop — no API keys needed
								</p>
							</div>
							<div class="mt-1">
								{#if providerChoice === 'janus'}
									<div class="w-5 h-5 rounded-full bg-primary flex items-center justify-center">
										<Check class="w-3 h-3 text-primary-content" />
									</div>
								{:else}
									<div class="w-5 h-5 rounded-full border-2 border-base-300"></div>
								{/if}
							</div>
						</div>
					</button>

					<!-- More options toggle -->
					<button
						type="button"
						class="w-full text-sm text-base-content/50 hover:text-base-content/70 flex items-center justify-center gap-1 py-2"
						onclick={() => (showMoreProviders = !showMoreProviders)}
					>
						Use your own API key or CLI instead
						<ChevronDown class="w-4 h-4 transition-transform {showMoreProviders ? 'rotate-180' : ''}" />
					</button>

					<!-- Expanded: CLI + API Key options -->
					{#if showMoreProviders}
						<div class="space-y-3">
							{#if isCheckingCLI}
								<div class="flex items-center justify-center py-4">
									<Loader2 class="w-5 h-5 animate-spin text-base-content/40" />
									<span class="ml-2 text-sm text-base-content/40">Detecting CLI tools...</span>
								</div>
							{:else}
								<!-- Show all authenticated CLIs -->
								{#each authenticatedCLIs() as cliKey (cliKey)}
									{@const info = cliProviderInfo[cliKey]}
									{@const status = cliStatuses?.[cliKey as keyof components.CLIStatusMap]}
									<button
										type="button"
										class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice === info.id
											? 'border-primary bg-primary/5'
											: 'border-base-300 hover:border-base-content/30'}"
										onclick={() => (providerChoice = info.id as ProviderChoice)}
									>
										<div class="flex items-start gap-3">
											<div class="p-2 rounded-lg bg-success/20">
												<Terminal class="w-5 h-5 text-success" />
											</div>
											<div class="flex-1">
												<div class="flex items-center gap-2">
													<span class="font-semibold">{info.name}</span>
													<span class="badge badge-success badge-sm">Ready</span>
												</div>
												<p class="text-sm text-base-content/60 mt-1">
													{info.description}
												</p>
												{#if status?.version}
													<p class="text-xs text-base-content/40 mt-1">
														v{status.version}
													</p>
												{/if}
											</div>
											<div class="mt-1">
												{#if providerChoice === info.id}
													<div class="w-5 h-5 rounded-full bg-primary flex items-center justify-center">
														<Check class="w-3 h-3 text-primary-content" />
													</div>
												{:else}
													<div class="w-5 h-5 rounded-full border-2 border-base-300"></div>
												{/if}
											</div>
										</div>
									</button>
								{/each}

								<!-- Show installed but not authenticated CLIs as "needs setup" -->
								{#if cliStatuses}
									{#each Object.entries(cliStatuses) as [cliKey, status]}
										{#if status?.installed && !status?.authenticated}
											{@const info = cliProviderInfo[cliKey]}
											<div class="w-full p-4 rounded-xl border-2 border-base-300 bg-base-200/50 text-left opacity-60">
												<div class="flex items-start gap-3">
													<div class="p-2 rounded-lg bg-warning/20">
														<Terminal class="w-5 h-5 text-warning" />
													</div>
													<div class="flex-1">
														<div class="flex items-center gap-2">
															<span class="font-semibold">{info.name}</span>
															<span class="badge badge-warning badge-sm">Needs Login</span>
														</div>
														<p class="text-sm text-base-content/60 mt-1">
															Installed but not logged in. Run <code class="text-xs bg-base-300 px-1 rounded">{cliKey}</code> in terminal to authenticate.
														</p>
													</div>
												</div>
											</div>
										{/if}
									{/each}
								{/if}
							{/if}

							<!-- API Key Option -->
							<button
								type="button"
								class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice === 'api-key'
									? 'border-primary bg-primary/5'
									: 'border-base-300 hover:border-base-content/30'}"
								onclick={() => (providerChoice = 'api-key')}
							>
								<div class="flex items-start gap-3">
									<div class="p-2 rounded-lg bg-secondary/20">
										<Key class="w-5 h-5 text-secondary" />
									</div>
									<div class="flex-1">
										<div class="flex items-center gap-2">
											<span class="font-semibold">Add API Key</span>
										</div>
										<p class="text-sm text-base-content/60 mt-1">
											Use an API key from Anthropic, OpenAI, or Google.
										</p>
									</div>
									<div class="mt-1">
										{#if providerChoice === 'api-key'}
											<div class="w-5 h-5 rounded-full bg-primary flex items-center justify-center">
												<Check class="w-3 h-3 text-primary-content" />
											</div>
										{:else}
											<div class="w-5 h-5 rounded-full border-2 border-base-300"></div>
										{/if}
									</div>
								</div>
							</button>
						</div>
					{/if}
				</div>

				<Button
					type="primary"
					class="w-full"
					onclick={handleProviderChoiceContinue}
					disabled={isSettingUpCLI || !providerChoice}
				>
					{#if isSettingUpCLI}
						<Loader2 class="w-5 h-5 mr-2 animate-spin" />
						Setting up...
					{:else}
						Continue
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- API Key Step -->
		{#if currentStep === 'api-key'}
			<div class="animate-in fade-in duration-300">
				<div class="w-16 h-16 rounded-full bg-secondary/20 flex items-center justify-center mx-auto mb-6">
					<Key class="w-8 h-8 text-secondary" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">Enter API Key</h2>
				<p class="text-base-content/70 text-center mb-6">
					Your key is stored securely and never leaves your device.
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				{#if keyValid}
					<div class="alert alert-success mb-4">
						<Check class="w-5 h-5" />
						<span>API key verified successfully!</span>
					</div>
				{/if}

				<div class="space-y-4">
					<div>
						<label class="label" for="provider-select">
							<span class="label-text">Provider</span>
						</label>
						<select
							id="provider-select"
							class="select select-bordered w-full"
							bind:value={provider}
							disabled={isTestingKey}
						>
							<option value="anthropic">Anthropic (Claude) - Recommended</option>
							<option value="openai">OpenAI (GPT)</option>
							<option value="google">Google (Gemini)</option>
						</select>
					</div>

					<div>
						<label class="label" for="api-key-input">
							<span class="label-text">API Key</span>
						</label>
						<input
							id="api-key-input"
							type="password"
							class="input input-bordered w-full"
							placeholder={providerInfo[provider].placeholder}
							bind:value={apiKey}
							disabled={isTestingKey}
						/>
						<label class="label">
							<a
								href={providerInfo[provider].helpUrl}
								target="_blank"
								rel="noopener noreferrer"
								class="label-text-alt link link-primary"
							>
								Get an API key
							</a>
						</label>
					</div>

					<Button
						type="primary"
						class="w-full"
						onclick={testAndSaveApiKey}
						disabled={isTestingKey || !apiKey.trim()}
					>
						{#if isTestingKey}
							<Loader2 class="w-5 h-5 mr-2 animate-spin" />
							Verifying...
						{:else}
							Continue
							<ArrowRight class="w-5 h-5 ml-2" />
						{/if}
					</Button>

					<button
						type="button"
						class="w-full text-sm text-base-content/60 hover:text-base-content"
						onclick={() => (currentStep = 'provider-choice')}
					>
						← Back to provider selection
					</button>
				</div>
			</div>
		{/if}

		<!-- Capabilities Step -->
		{#if currentStep === 'capabilities'}
			<div class="animate-in fade-in duration-300">
				<div class="w-16 h-16 rounded-full bg-info/20 flex items-center justify-center mx-auto mb-6">
					<Shield class="w-8 h-8 text-info" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">Agent Capabilities</h2>
				<p class="text-base-content/70 text-center mb-6">
					Choose what Nebo can access. You can change these anytime in Settings.
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				<div class="space-y-2 mb-6 max-h-72 overflow-y-auto">
					{#each capabilityGroups as cap}
						<button
							type="button"
							class="w-full p-3 rounded-lg border transition-all text-left
								{permissions[cap.key]
									? 'border-primary/30 bg-primary/5'
									: 'border-base-300 hover:border-base-content/20'}
								{cap.alwaysOn ? 'opacity-80 cursor-default' : ''}"
							onclick={() => togglePermission(cap.key)}
							disabled={cap.alwaysOn}
						>
							<div class="flex items-center gap-3">
								<div class="p-1.5 rounded-lg {permissions[cap.key] ? 'bg-primary/20' : 'bg-base-200'}">
									<cap.icon class="w-4 h-4 {permissions[cap.key] ? 'text-primary' : 'text-base-content/50'}" />
								</div>
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2">
										<span class="font-medium text-sm">{cap.label}</span>
										{#if cap.alwaysOn}
											<span class="badge badge-neutral badge-xs">Always on</span>
										{/if}
									</div>
									<p class="text-xs text-base-content/50 truncate">{cap.description}</p>
								</div>
								<input
									type="checkbox"
									class="toggle toggle-primary toggle-sm"
									checked={permissions[cap.key]}
									disabled={cap.alwaysOn}
									onclick={(e: MouseEvent) => e.stopPropagation()}
									onchange={() => togglePermission(cap.key)}
								/>
							</div>
						</button>
					{/each}
				</div>

				<Button
					type="primary"
					class="w-full"
					onclick={savePermissionsAndFinish}
					disabled={isSavingPermissions}
				>
					{#if isSavingPermissions}
						<Loader2 class="w-5 h-5 mr-2 animate-spin" />
						Saving...
					{:else}
						Continue
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- NeboLoop Step -->
		{#if currentStep === 'neboloop'}
			<div class="animate-in fade-in duration-300">
				<div class="w-16 h-16 rounded-full bg-accent/20 flex items-center justify-center mx-auto mb-6">
					<Store class="w-8 h-8 text-accent" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">
					{cameFromJanus ? 'Create Your Account' : 'NeboLoop'}
				</h2>
				<p class="text-base-content/70 text-center mb-6">
					{cameFromJanus
						? 'Sign up or log in to NeboLoop to use Janus AI.'
						: 'Connect to the NeboLoop marketplace to install apps, skills, and AI providers.'}
				</p>

				{#if neboLoopConnected}
					<div class="flex flex-col items-center gap-4 py-6 mb-6">
						<CircleCheck class="h-12 w-12 text-success" />
						<p class="text-lg font-medium">Connected to NeboLoop</p>
						<p class="text-base-content/70">{neboLoopEmail}</p>
					</div>
					<Button type="primary" class="w-full" onclick={() => {
						if (cameFromJanus) {
							currentStep = 'capabilities';
						} else {
							completeOnboarding();
						}
					}}>
						Continue
						<ArrowRight class="w-5 h-5 ml-2" />
					</Button>
				{:else}
					<div role="tablist" class="tabs tabs-bordered mb-6">
						<button
							role="tab"
							class="tab"
							class:tab-active={neboLoopTab === 'signup'}
							onclick={() => { neboLoopTab = 'signup'; neboLoopError = ''; }}
						>
							Sign Up
						</button>
						<button
							role="tab"
							class="tab"
							class:tab-active={neboLoopTab === 'login'}
							onclick={() => { neboLoopTab = 'login'; neboLoopError = ''; }}
						>
							Log In
						</button>
					</div>

					{#if neboLoopError}
						<div class="alert alert-error mb-4">
							<span>{neboLoopError}</span>
						</div>
					{/if}

					{#if neboLoopTab === 'signup'}
						<form onsubmit={(e) => { e.preventDefault(); handleNeboLoopSignup(); }} class="space-y-3">
							<input
								type="text"
								placeholder="Display Name"
								class="input input-bordered w-full"
								bind:value={signupName}
								disabled={neboLoopLoading}
							/>
							<input
								type="email"
								placeholder="Email"
								class="input input-bordered w-full"
								bind:value={signupEmail}
								disabled={neboLoopLoading}
							/>
							<input
								type="password"
								placeholder="Password"
								class="input input-bordered w-full"
								bind:value={signupPassword}
								disabled={neboLoopLoading}
							/>
							<input
								type="password"
								placeholder="Confirm Password"
								class="input input-bordered w-full"
								bind:value={signupConfirm}
								disabled={neboLoopLoading}
							/>
							<Button
								type="primary"
								class="w-full"
								onclick={handleNeboLoopSignup}
								disabled={neboLoopLoading}
							>
								{#if neboLoopLoading}
									<Loader2 class="w-5 h-5 mr-2 animate-spin" />
									Creating Account...
								{:else}
									Create Account
									<ArrowRight class="w-5 h-5 ml-2" />
								{/if}
							</Button>
						</form>
					{:else}
						<form onsubmit={(e) => { e.preventDefault(); handleNeboLoopLogin(); }} class="space-y-3">
							<input
								type="email"
								placeholder="Email"
								class="input input-bordered w-full"
								bind:value={nlLoginEmail}
								disabled={neboLoopLoading}
							/>
							<input
								type="password"
								placeholder="Password"
								class="input input-bordered w-full"
								bind:value={nlLoginPassword}
								disabled={neboLoopLoading}
							/>
							<Button
								type="primary"
								class="w-full"
								onclick={handleNeboLoopLogin}
								disabled={neboLoopLoading}
							>
								{#if neboLoopLoading}
									<Loader2 class="w-5 h-5 mr-2 animate-spin" />
									Logging in...
								{:else}
									Log In
									<ArrowRight class="w-5 h-5 ml-2" />
								{/if}
							</Button>
						</form>
					{/if}

					<div class="flex justify-between mt-4">
						<button
							type="button"
							class="text-sm text-base-content/60 hover:text-base-content"
							onclick={() => {
								if (cameFromJanus) {
									cameFromJanus = false;
									currentStep = 'provider-choice';
								} else {
									currentStep = 'capabilities';
								}
							}}
						>
							← Back
						</button>
						{#if !cameFromJanus}
							<button
								type="button"
								class="text-sm text-base-content/60 hover:text-base-content"
								onclick={completeOnboarding}
							>
								Skip for now →
							</button>
						{/if}
					</div>
				{/if}
			</div>
		{/if}

		<!-- Complete Step -->
		{#if currentStep === 'complete'}
			<div class="text-center animate-in fade-in duration-300">
				<div class="w-20 h-20 rounded-full bg-success/20 flex items-center justify-center mx-auto mb-6">
					<Check class="w-10 h-10 text-success" />
				</div>
				<h2 class="text-3xl font-bold mb-3">You're all set!</h2>
				<p class="text-base-content/70 mb-8 text-lg">
					Nebo is ready to meet you. Let's chat!
				</p>
				<Button type="primary" size="lg" onclick={finishOnboarding}>
					Start Chatting
					<ArrowRight class="w-5 h-5 ml-2" />
				</Button>
			</div>
		{/if}
	</div>
</div>
