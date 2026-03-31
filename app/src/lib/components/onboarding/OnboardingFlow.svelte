<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { t, locale } from 'svelte-i18n';
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
	import {
		neboLoopOAuthStartWithJanus,
		neboLoopOAuthStatus,
		neboLoopAccountStatus
	} from '$lib/api';
	import type * as components from '$lib/api/neboComponents';
	import Button from '$lib/components/ui/Button.svelte';

	let { onComplete = () => {} }: { onComplete?: () => void } = $props();

	type OnboardingStep =
		| 'language'
		| 'welcome'
		| 'terms'
		| 'provider-choice'
		| 'api-key'
		| 'capabilities'
		| 'neboloop'
		| 'complete';
	type ProviderChoice = 'janus' | 'claude-code' | 'codex-cli' | 'gemini-cli' | 'api-key';

	let currentStep = $state<OnboardingStep>('language');
	let error = $state('');
	let selectedLanguage = $state('en');
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
		file: true,
		shell: false,
		web: true,
		contacts: false,
		desktop: true,
		media: false,
		system: true
	});
	let isSavingPermissions = $state(false);

	// NeboLoop
	let neboLoopLoading = $state(false);
	let neboLoopError = $state('');
	let neboLoopConnected = $state(false);
	let neboLoopEmail = $state('');
	let neboLoopPendingState = $state('');
	let neboLoopPollTimer = $state<ReturnType<typeof setInterval> | null>(null);

	const onboardingLanguages = [
		{ value: 'en', label: 'English' },
		{ value: 'de', label: 'Deutsch' },
		{ value: 'es', label: 'Español' },
		{ value: 'fr', label: 'Français' },
		{ value: 'it', label: 'Italiano' },
		{ value: 'pt-BR', label: 'Português (Brasil)' },
		{ value: 'nl', label: 'Nederlands' },
		{ value: 'pl', label: 'Polski' },
		{ value: 'tr', label: 'Türkçe' },
		{ value: 'uk', label: 'Українська' },
		{ value: 'vi', label: 'Tiếng Việt' },
		{ value: 'ar', label: 'العربية' },
		{ value: 'hi', label: 'हिन्दी' },
		{ value: 'ja', label: '日本語' },
		{ value: 'ko', label: '한국어' },
		{ value: 'zh-CN', label: '中文 (简体)' },
		{ value: 'zh-TW', label: '中文 (繁體)' }
	];

	function selectLanguage(lang: string) {
		selectedLanguage = lang;
		locale.set(lang);
		localStorage.setItem('nebo_locale', lang);
		if (typeof document !== 'undefined') {
			document.documentElement.dir = lang === 'ar' ? 'rtl' : 'ltr';
			document.documentElement.lang = lang;
		}
	}

	async function saveLanguageAndContinue() {
		try {
			await api.updatePreferences({ language: selectedLanguage, emailNotifications: false, marketingEmails: false });
		} catch {
			// Non-blocking — continue even if save fails
		}
		currentStep = 'welcome';
	}

	const capabilityGroups = [
		{
			key: 'chat',
			labelKey: 'onboarding.capabilityNames.chat',
			descKey: 'onboarding.capabilityNames.chatDesc',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			labelKey: 'onboarding.capabilityNames.filesystem',
			descKey: 'onboarding.capabilityNames.filesystemDesc',
			icon: FileText
		},
		{
			key: 'shell',
			labelKey: 'onboarding.capabilityNames.shell',
			descKey: 'onboarding.capabilityNames.shellDesc',
			icon: Terminal
		},
		{
			key: 'web',
			labelKey: 'onboarding.capabilityNames.web',
			descKey: 'onboarding.capabilityNames.webDesc',
			icon: Globe
		},
		{
			key: 'contacts',
			labelKey: 'onboarding.capabilityNames.contacts',
			descKey: 'onboarding.capabilityNames.contactsDesc',
			icon: Users
		},
		{
			key: 'desktop',
			labelKey: 'onboarding.capabilityNames.desktop',
			descKey: 'onboarding.capabilityNames.desktopDesc',
			icon: Monitor
		},
		{
			key: 'media',
			labelKey: 'onboarding.capabilityNames.media',
			descKey: 'onboarding.capabilityNames.mediaDesc',
			icon: Camera
		},
		{
			key: 'system',
			labelKey: 'onboarding.capabilityNames.system',
			descKey: 'onboarding.capabilityNames.systemDesc',
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

	// CLI provider info loaded from models.yaml via API (no hardcoded model IDs)
	let cliProviderInfo = $state<
		Record<string, { id: string; name: string; descriptionKey: string; model: string }>
	>({});

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
	const progressSteps = [
		'language',
		'welcome',
		'terms',
		'provider-choice',
		'capabilities',
		'neboloop',
		'complete'
	];

	// CLI command → i18n key (static, not model-dependent)
	const cliDescriptionKeys: Record<string, string> = {
		claude: 'onboarding.provider.cliClaude',
		codex: 'onboarding.provider.cliCodex',
		gemini: 'onboarding.provider.cliGemini'
	};

	onMount(async () => {
		// Auto-detect language from browser/OS
		const browserLang = navigator.language?.split('-')[0];
		const match = onboardingLanguages.find(l => l.value === navigator.language) ||
			onboardingLanguages.find(l => l.value.startsWith(browserLang));
		if (match) {
			selectLanguage(match.value);
		}

		// Check CLI statuses and load CLI provider config from models.yaml
		try {
			const response = await api.listModels();
			cliStatuses = response.cliStatuses ?? null;

			// Build cliProviderInfo from API response (models come from models.yaml)
			if (response.cliProviders) {
				const info: Record<
					string,
					{ id: string; name: string; descriptionKey: string; model: string }
				> = {};
				for (const cp of response.cliProviders) {
					// defaultModel comes from models.yaml cli_providers section
					const defaultModel = cp.defaultModel || (cp.models?.[0] ?? '');
					info[cp.command] = {
						id: cp.id,
						name: cp.displayName,
						descriptionKey: cliDescriptionKeys[cp.command] ?? 'onboarding.cliProviderUse',
						model: defaultModel ? `${cp.id}/${defaultModel}` : cp.id
					};
				}
				cliProviderInfo = info;
			}
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
			error = err?.message || $t('onboarding.termsFailed');
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
			error = err?.message || $t('onboarding.configFailed', { values: { name: cliProviderInfo[cliKey].name } });
		} finally {
			isSettingUpCLI = false;
		}
	}

	async function testAndSaveApiKey() {
		if (!apiKey.trim()) {
			error = $t('onboarding.apiKey.pleaseEnterKey');
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
				error = testResponse.message || $t('onboarding.apiKey.validationFailed');
				// Delete the invalid profile
				await api.deleteAuthProfile(profileResponse.profile.id);
			}
		} catch (err: any) {
			error = err?.message || $t('onboarding.apiKey.saveFailed');
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
			error = err?.message || $t('onboarding.permissionsFailed');
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

	async function startNeboLoopOAuth() {
		neboLoopError = '';
		neboLoopLoading = true;
		try {
			const { state } = await neboLoopOAuthStartWithJanus(cameFromJanus);
			neboLoopPendingState = state;
			// Server opens the OAuth URL in the system browser via open::that()

			// Auto-timeout after 3 minutes
			const timeout = setTimeout(
				() => {
					if (neboLoopLoading) {
						cleanupNeboLoopOAuth();
						neboLoopError = $t('onboarding.neboloop.signInTimeout');
						neboLoopLoading = false;
					}
				},
				3 * 60 * 1000
			);

			// Poll status until the OAuth flow completes in the browser
			neboLoopPollTimer = setInterval(async () => {
				try {
					const result = await neboLoopOAuthStatus({ state: neboLoopPendingState });
					if (result.status === 'complete') {
						clearTimeout(timeout);
						cleanupNeboLoopOAuth();
						neboLoopConnected = true;
						neboLoopEmail = result.email ?? '';
						neboLoopLoading = false;
					} else if (result.status === 'error') {
						clearTimeout(timeout);
						cleanupNeboLoopOAuth();
						neboLoopError = result.error ?? $t('onboarding.neboloop.signInFailed');
						neboLoopLoading = false;
					} else if (result.status === 'expired') {
						clearTimeout(timeout);
						cleanupNeboLoopOAuth();
						neboLoopError = $t('onboarding.neboloop.signInExpired');
						neboLoopLoading = false;
					}
				} catch {
					// polling error, keep trying
				}
			}, 2000);
		} catch (e: any) {
			neboLoopError = e?.message || $t('onboarding.neboloop.startFailed');
			neboLoopLoading = false;
		}
	}

	function cleanupNeboLoopOAuth() {
		if (neboLoopPollTimer) {
			clearInterval(neboLoopPollTimer);
			neboLoopPollTimer = null;
		}
		neboLoopPendingState = '';
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
		onComplete();
		goto('/agent/assistant/chat');
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

<div class="fixed inset-0 bg-base-100 z-50 scrollbar-overlay">
	<div class="w-full max-w-lg mx-auto py-8 px-8 min-h-full flex flex-col justify-center">
		<!-- Progress dots -->
		<div class="flex justify-center gap-2 mb-8">
			{#each progressSteps as step}
				{@const stepIndex = progressSteps.indexOf(
					currentStep === 'api-key' ? 'provider-choice' : currentStep
				)}
				{@const dotIndex = progressSteps.indexOf(step)}
				<div
					class="w-2 h-2 rounded-full transition-colors {stepIndex >= dotIndex
						? 'bg-primary'
						: 'bg-base-300'}"
				></div>
			{/each}
		</div>

		<!-- Language Step -->
		{#if currentStep === 'language'}
			<div class="text-center animate-in fade-in duration-300">
				<div class="w-20 h-20 rounded-full bg-primary/20 flex items-center justify-center mx-auto mb-6">
					<Globe class="w-10 h-10 text-primary" />
				</div>
				<h1 class="text-3xl font-bold mb-3">{$t('onboarding.language.title')}</h1>
				<p class="text-base-content/80 mb-6 text-lg">{$t('onboarding.language.description')}</p>
				<div class="grid grid-cols-2 gap-2 max-w-md mx-auto mb-8 px-1">
					{#each onboardingLanguages as lang}
						<button
							type="button"
							onclick={() => selectLanguage(lang.value)}
							class="flex items-center justify-between gap-2 px-4 py-3 rounded-xl border transition-all text-left
								{selectedLanguage === lang.value
									? 'bg-primary/10 border-primary/30 text-primary'
									: 'bg-base-content/5 border-transparent text-base-content/90 hover:border-base-content/15'}"
						>
							<span class="text-base font-medium">{lang.label}</span>
							{#if selectedLanguage === lang.value}
								<Check class="w-4 h-4 shrink-0" />
							{/if}
						</button>
					{/each}
				</div>
				<Button type="primary" size="lg" onclick={saveLanguageAndContinue}>
					{$t('common.continue')}
					<ArrowRight class="w-5 h-5 ml-2" />
				</Button>
			</div>
		{/if}

		<!-- Welcome Step -->
		{#if currentStep === 'welcome'}
			<div class="text-center animate-in fade-in duration-300">
				<div
					class="w-20 h-20 rounded-full bg-primary/20 flex items-center justify-center mx-auto mb-6"
				>
					<Sparkles class="w-10 h-10 text-primary" />
				</div>
				<h1 class="text-3xl font-bold mb-3">{$t('onboarding.welcome.title')}</h1>
				<p class="text-base-content/80 mb-8 text-lg">
					{$t('onboarding.welcome.description')}
				</p>
				<Button type="primary" size="lg" onclick={() => (currentStep = 'terms')}>
					{$t('onboarding.welcome.getStarted')}
					<ArrowRight class="w-5 h-5 ml-2" />
				</Button>
			</div>
		{/if}

		<!-- Terms Step -->
		{#if currentStep === 'terms'}
			<div class="animate-in fade-in duration-300">
				<div
					class="w-16 h-16 rounded-full bg-warning/20 flex items-center justify-center mx-auto mb-6"
				>
					<Shield class="w-8 h-8 text-warning" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">{$t('onboarding.terms.title')}</h2>
				<p class="text-base-content/80 text-center mb-6">
					{$t('onboarding.terms.description')}
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				<div class="bg-base-200 rounded-xl p-5 mb-6 space-y-4 text-base max-h-64 scrollbar-overlay">
					<div>
						<h3 class="font-semibold text-base-content mb-1">{$t('onboarding.terms.dataLocal')}</h3>
						<p class="text-base-content/90">
							{$t('onboarding.terms.dataLocalDesc')}
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">{$t('onboarding.terms.aiProvider')}</h3>
						<p class="text-base-content/90">
							{$t('onboarding.terms.aiProviderDesc')}
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">{$t('onboarding.terms.apiKeys')}</h3>
						<p class="text-base-content/90">
							{$t('onboarding.terms.apiKeysDesc')}
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">{$t('onboarding.terms.systemAccess')}</h3>
						<p class="text-base-content/90">
							{$t('onboarding.terms.systemAccessDesc')}
						</p>
					</div>
					<div>
						<h3 class="font-semibold text-base-content mb-1">{$t('onboarding.terms.noAnalytics')}</h3>
						<p class="text-base-content/90">
							{$t('onboarding.terms.noAnalyticsDesc')}
						</p>
					</div>
				</div>

				<label class="flex items-start gap-3 mb-6 cursor-pointer">
					<input
						type="checkbox"
						class="checkbox checkbox-primary mt-0.5"
						bind:checked={termsAccepted}
					/>
					<span class="text-base text-base-content/80">
						{$t('onboarding.terms.consent')}
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
						{$t('common.saving')}
					{:else}
						{$t('common.continue')}
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- Provider Choice Step -->
		{#if currentStep === 'provider-choice'}
			<div class="animate-in fade-in duration-300 flex flex-col max-h-[85vh]">
				<div
					class="w-16 h-16 rounded-full bg-primary/20 flex items-center justify-center mx-auto mb-6 shrink-0"
				>
					<Sparkles class="w-8 h-8 text-primary" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2 shrink-0">{$t('onboarding.provider.title')}</h2>
				<p class="text-base-content/90 text-center mb-6 shrink-0">{$t('onboarding.provider.subtitle')}</p>

				{#if error}
					<div class="alert alert-error mb-4 shrink-0">
						<span>{error}</span>
					</div>
				{/if}

				<div class="space-y-3 mb-6 shrink-0">
					<!-- Janus - Primary option -->
					<button
						type="button"
						class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice ===
						'janus'
							? 'border-primary bg-primary/5'
							: 'border-base-300 hover:border-base-content/40'}"
						onclick={() => (providerChoice = 'janus')}
					>
						<div class="flex items-start gap-3">
							<div class="p-2 rounded-lg bg-primary/20">
								<Sparkles class="w-5 h-5 text-primary" />
							</div>
							<div class="flex-1">
								<div class="flex items-center gap-2">
									<span class="font-semibold">{$t('onboarding.provider.janus')}</span>
									<span class="badge badge-primary badge-sm">{$t('onboarding.provider.recommended')}</span>
								</div>
								<p class="text-base text-base-content/80 mt-1">
									{$t('onboarding.provider.janusDesc')}
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
						class="w-full text-base text-base-content/90 hover:text-base-content/90 flex items-center justify-center gap-1 py-2"
						onclick={() => (showMoreProviders = !showMoreProviders)}
					>
						{$t('onboarding.provider.useOwnKey')}
						<ChevronDown
							class="w-4 h-4 transition-transform {showMoreProviders ? 'rotate-180' : ''}"
						/>
					</button>
				</div>

				<!-- Expanded: CLI + API Key options -->
				{#if showMoreProviders}
					<div class="space-y-3 mb-6 scrollbar-overlay min-h-0">
						{#if isCheckingCLI}
							<div class="flex items-center justify-center py-4">
								<Loader2 class="w-5 h-5 animate-spin text-base-content/90" />
								<span class="ml-2 text-base text-base-content/90">{$t('onboarding.provider.detectingCli')}</span>
							</div>
						{:else}
							<!-- Show all authenticated CLIs -->
							{#each authenticatedCLIs() as cliKey (cliKey)}
								{@const info = cliProviderInfo[cliKey]}
								{@const status = cliStatuses?.[cliKey as keyof components.CLIStatusMap]}
								<button
									type="button"
									class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice ===
									info.id
										? 'border-primary bg-primary/5'
										: 'border-base-300 hover:border-base-content/40'}"
									onclick={() => (providerChoice = info.id as ProviderChoice)}
								>
									<div class="flex items-start gap-3">
										<div class="p-2 rounded-lg bg-success/20">
											<Terminal class="w-5 h-5 text-success" />
										</div>
										<div class="flex-1">
											<div class="flex items-center gap-2">
												<span class="font-semibold">{info.name}</span>
												<span class="badge badge-success badge-sm">{$t('onboarding.provider.ready')}</span>
											</div>
											<p class="text-base text-base-content/90 mt-1">
												{$t(info.descriptionKey, { values: { name: info.name } })}
											</p>
											{#if status?.version}
												<p class="text-sm text-base-content/90 mt-1">
													v{status.version}
												</p>
											{/if}
										</div>
										<div class="mt-1">
											{#if providerChoice === info.id}
												<div
													class="w-5 h-5 rounded-full bg-primary flex items-center justify-center"
												>
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
										<div
											class="w-full p-4 rounded-xl border-2 border-base-300 bg-base-200/50 text-left opacity-60"
										>
											<div class="flex items-start gap-3">
												<div class="p-2 rounded-lg bg-warning/20">
													<Terminal class="w-5 h-5 text-warning" />
												</div>
												<div class="flex-1">
													<div class="flex items-center gap-2">
														<span class="font-semibold">{info.name}</span>
														<span class="badge badge-warning badge-sm">{$t('onboarding.provider.needsLogin')}</span>
													</div>
													<p class="text-base text-base-content/90 mt-1">
														{$t('onboarding.provider.needsLoginDesc', { values: { cliKey } })}
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
							class="w-full p-4 rounded-xl border-2 transition-all text-left {providerChoice ===
							'api-key'
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/40'}"
							onclick={() => (providerChoice = 'api-key')}
						>
							<div class="flex items-start gap-3">
								<div class="p-2 rounded-lg bg-secondary/20">
									<Key class="w-5 h-5 text-secondary" />
								</div>
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="font-semibold">{$t('onboarding.provider.addApiKey')}</span>
									</div>
									<p class="text-base text-base-content/90 mt-1">
										{$t('onboarding.provider.addApiKeyDesc')}
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

				<Button
					type="primary"
					class="w-full"
					onclick={handleProviderChoiceContinue}
					disabled={isSettingUpCLI || !providerChoice}
				>
					{#if isSettingUpCLI}
						<Loader2 class="w-5 h-5 mr-2 animate-spin" />
						{$t('onboarding.provider.settingUp')}
					{:else}
						{$t('common.continue')}
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- API Key Step -->
		{#if currentStep === 'api-key'}
			<div class="animate-in fade-in duration-300">
				<div
					class="w-16 h-16 rounded-full bg-secondary/20 flex items-center justify-center mx-auto mb-6"
				>
					<Key class="w-8 h-8 text-secondary" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">{$t('onboarding.apiKey.title')}</h2>
				<p class="text-base-content/90 text-center mb-6">
					{$t('onboarding.apiKey.description')}
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				{#if keyValid}
					<div class="alert alert-success mb-4">
						<Check class="w-5 h-5" />
						<span>{$t('onboarding.apiKey.verified')}</span>
					</div>
				{/if}

				<div class="space-y-4">
					<div>
						<label class="label" for="provider-select">
							<span class="label-text">{$t('onboarding.apiKey.providerLabel')}</span>
						</label>
						<select
							id="provider-select"
							class="select select-bordered w-full"
							bind:value={provider}
							disabled={isTestingKey}
						>
							<option value="anthropic">{$t('onboarding.apiKey.anthropic')}</option>
							<option value="openai">{$t('onboarding.apiKey.openai')}</option>
							<option value="google">{$t('onboarding.apiKey.google')}</option>
						</select>
					</div>

					<div>
						<label class="label" for="api-key-input">
							<span class="label-text">{$t('onboarding.apiKey.apiKeyLabel')}</span>
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
								{$t('onboarding.apiKey.getKey')}
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
							{$t('onboarding.apiKey.verifying')}
						{:else}
							{$t('common.continue')}
							<ArrowRight class="w-5 h-5 ml-2" />
						{/if}
					</Button>

					<button
						type="button"
						class="w-full text-base text-base-content/90 hover:text-base-content"
						onclick={() => (currentStep = 'provider-choice')}
					>
						{$t('onboarding.apiKey.backToProvider')}
					</button>
				</div>
			</div>
		{/if}

		<!-- Capabilities Step -->
		{#if currentStep === 'capabilities'}
			<div class="animate-in fade-in duration-300">
				<div
					class="w-16 h-16 rounded-full bg-info/20 flex items-center justify-center mx-auto mb-6"
				>
					<Shield class="w-8 h-8 text-info" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">{$t('onboarding.capabilities.title')}</h2>
				<p class="text-base-content/90 text-center mb-6">
					{$t('onboarding.capabilities.description')}
				</p>

				{#if error}
					<div class="alert alert-error mb-4">
						<span>{error}</span>
					</div>
				{/if}

				<div class="space-y-2 mb-6 max-h-72 scrollbar-overlay">
					{#each capabilityGroups as cap}
						<button
							type="button"
							class="w-full p-3 rounded-lg border transition-all text-left
								{permissions[cap.key]
								? 'border-primary/30 bg-primary/5'
								: 'border-base-300 hover:border-base-content/40'}
								{cap.alwaysOn ? 'opacity-80 cursor-default' : ''}"
							onclick={() => togglePermission(cap.key)}
							disabled={cap.alwaysOn}
						>
							<div class="flex items-center gap-3">
								<div
									class="p-1.5 rounded-lg {permissions[cap.key] ? 'bg-primary/20' : 'bg-base-200'}"
								>
									<cap.icon
										class="w-4 h-4 {permissions[cap.key] ? 'text-primary' : 'text-base-content/90'}"
									/>
								</div>
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2">
										<span class="font-medium text-base">{$t(cap.labelKey)}</span>
										{#if cap.alwaysOn}
											<span class="badge badge-neutral badge-xs">{$t('onboarding.capabilities.alwaysOn')}</span>
										{/if}
									</div>
									<p class="text-sm text-base-content/90 truncate">{$t(cap.descKey)}</p>
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
						{$t('common.saving')}
					{:else}
						{$t('common.continue')}
						<ArrowRight class="w-5 h-5 ml-2" />
					{/if}
				</Button>
			</div>
		{/if}

		<!-- NeboLoop Step -->
		{#if currentStep === 'neboloop'}
			<div class="animate-in fade-in duration-300">
				<div
					class="w-16 h-16 rounded-full bg-accent/20 flex items-center justify-center mx-auto mb-6"
				>
					<Store class="w-8 h-8 text-accent" />
				</div>
				<h2 class="text-2xl font-bold text-center mb-2">
					{cameFromJanus ? $t('onboarding.neboloop.connectTitle') : $t('onboarding.neboloop.title')}
				</h2>
				<p class="text-base-content/90 text-center mb-6">
					{cameFromJanus
						? $t('onboarding.neboloop.janusDesc')
						: $t('onboarding.neboloop.marketplaceDesc')}
				</p>

				{#if neboLoopConnected}
					<div class="flex flex-col items-center gap-4 py-6 mb-6">
						<CircleCheck class="h-12 w-12 text-success" />
						<p class="text-lg font-medium">{$t('onboarding.neboloop.connected')}</p>
						<p class="text-base-content/90">{neboLoopEmail}</p>
					</div>
					<Button
						type="primary"
						class="w-full"
						onclick={() => {
							if (cameFromJanus) {
								currentStep = 'capabilities';
							} else {
								completeOnboarding();
							}
						}}
					>
						{$t('common.continue')}
						<ArrowRight class="w-5 h-5 ml-2" />
					</Button>
				{:else}
					{#if neboLoopError}
						<div class="alert alert-error mb-4">
							<span>{neboLoopError}</span>
						</div>
					{/if}

					<div class="flex flex-col items-center gap-4 py-4">
						<p class="text-base-content/90 text-center text-base max-w-sm">
							{$t('onboarding.neboloop.signInDesc')}
						</p>
						<Button
							type="primary"
							size="lg"
							onclick={startNeboLoopOAuth}
							disabled={neboLoopLoading}
						>
							{#if neboLoopLoading}
								<Loader2 class="w-5 h-5 mr-2 animate-spin" />
								{$t('onboarding.neboloop.waitingForSignIn')}
							{:else}
								{$t('onboarding.neboloop.continueWithNeboLoop')}
							{/if}
						</Button>
						{#if neboLoopLoading}
							<p class="text-base text-base-content/90">{$t('onboarding.neboloop.completeInBrowser')}</p>
							<button
								type="button"
								class="text-base text-base-content/90 hover:text-base-content underline"
								onclick={() => {
									cleanupNeboLoopOAuth();
									neboLoopLoading = false;
								}}
							>
								{$t('common.cancel')}
							</button>
						{/if}
					</div>

					<div class="flex justify-between mt-4">
						<button
							type="button"
							class="text-base text-base-content/90 hover:text-base-content"
							onclick={() => {
								if (cameFromJanus) {
									cameFromJanus = false;
									currentStep = 'provider-choice';
								} else {
									currentStep = 'capabilities';
								}
							}}
						>
							{$t('common.back')}
						</button>
						{#if !cameFromJanus}
							<button
								type="button"
								class="text-base text-base-content/90 hover:text-base-content"
								onclick={completeOnboarding}
							>
								{$t('onboarding.neboloop.skipForNow')}
							</button>
						{/if}
					</div>
				{/if}
			</div>
		{/if}

		<!-- Complete Step -->
		{#if currentStep === 'complete'}
			<div class="text-center animate-in fade-in duration-300">
				<div
					class="w-20 h-20 rounded-full bg-success/20 flex items-center justify-center mx-auto mb-6"
				>
					<Check class="w-10 h-10 text-success" />
				</div>
				<h2 class="text-3xl font-bold mb-3">{$t('onboarding.complete.title')}</h2>
				<p class="text-base-content/90 mb-8 text-lg">{$t('onboarding.complete.description')}</p>
				<Button type="primary" size="lg" onclick={finishOnboarding}>
					{$t('onboarding.complete.startChatting')}
					<ArrowRight class="w-5 h-5 ml-2" />
				</Button>
			</div>
		{/if}
	</div>
</div>
