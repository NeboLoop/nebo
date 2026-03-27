<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		FileText,
		Terminal,
		Globe,
		Users,
		Monitor,
		Camera,
		Cpu,
		MessageCircle,
		AlertTriangle
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';

	let isLoading = $state(true);
	let saveError = $state('');

	// Capability permissions
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

	// Agent settings
	let autonomousMode = $state(false);
	let autoApproveRead = $state(true);
	let autoApproveWrite = $state(false);
	let autoApproveBash = $state(false);

	// Preserved fields (not edited here, but must be included in save)
	let heartbeatIntervalMinutes = $state(30);
	let commEnabled = $state(false);
	let commPlugin = $state('');
	let developerMode = $state(false);

	// Terms modal state
	let showTermsModal = $state(false);
	let termsAccepted = $state(false);
	let confirmText = $state('');

	const canConfirmTerms = $derived(termsAccepted && confirmText === 'ENABLE');

	// --- Capability Groups ---
	const capabilityGroups = [
		{
			key: 'chat',
			labelKey: 'onboarding.capabilityNames.chat',
			descriptionKey: 'onboarding.capabilityNames.chatDesc',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			labelKey: 'onboarding.capabilityNames.filesystem',
			descriptionKey: 'onboarding.capabilityNames.filesystemDesc',
			icon: FileText
		},
		{
			key: 'shell',
			labelKey: 'onboarding.capabilityNames.shell',
			descriptionKey: 'onboarding.capabilityNames.shellDesc',
			icon: Terminal
		},
		{
			key: 'web',
			labelKey: 'onboarding.capabilityNames.web',
			descriptionKey: 'onboarding.capabilityNames.webDesc',
			icon: Globe
		},
		{
			key: 'contacts',
			labelKey: 'onboarding.capabilityNames.contacts',
			descriptionKey: 'onboarding.capabilityNames.contactsDesc',
			icon: Users
		},
		{
			key: 'desktop',
			labelKey: 'onboarding.capabilityNames.desktop',
			descriptionKey: 'onboarding.capabilityNames.desktopDesc',
			icon: Monitor
		},
		{
			key: 'media',
			labelKey: 'onboarding.capabilityNames.media',
			descriptionKey: 'onboarding.capabilityNames.mediaDesc',
			icon: Camera
		},
		{
			key: 'system',
			labelKey: 'onboarding.capabilityNames.system',
			descriptionKey: 'onboarding.capabilityNames.systemDesc',
			icon: Cpu
		}
	];

	// --- Loading ---
	onMount(async () => {
		try {
			const [settingsRes, permsRes] = await Promise.all([
				api.getAgentSettings(),
				api.getToolPermissions()
			]);

			const s = settingsRes.settings;
			autonomousMode = s.autonomousMode ?? false;
			autoApproveRead = s.autoApproveRead ?? true;
			autoApproveWrite = s.autoApproveWrite ?? false;
			autoApproveBash = s.autoApproveBash ?? false;
			heartbeatIntervalMinutes = s.heartbeatIntervalMinutes ?? 30;
			commEnabled = s.commEnabled ?? false;
			commPlugin = s.commPlugin ?? '';
			developerMode = s.developerMode ?? false;

			if (permsRes.permissions && Object.keys(permsRes.permissions).length > 0) {
				permissions = { ...permissions, ...permsRes.permissions };
			}
		} catch (err) {
			console.error('Failed to load settings:', err);
		} finally {
			isLoading = false;
		}
	});

	// --- Auto-save ---
	async function saveAll() {
		saveError = '';
		try {
			await Promise.all([
				api.updateToolPermissions({ permissions }),
				api.updateAgentSettings({
					autonomousMode,
					autoApproveRead,
					autoApproveWrite,
					autoApproveBash,
					heartbeatIntervalMinutes,
					commEnabled,
					commPlugin,
					developerMode
				})
			]);
		} catch (err: any) {
			saveError = err?.message || $t('common.failed');
			setTimeout(() => { saveError = ''; }, 4000);
		}
	}

	// --- Actions ---
	function togglePermission(key: string) {
		if (key === 'chat') return;
		if (autonomousMode) return;
		permissions = { ...permissions, [key]: !permissions[key] };
		saveAll();
	}

	function handleAutonomousChange() {
		if (autonomousMode) {
			autonomousMode = false;
			showTermsModal = true;
		} else {
			const defaults: Record<string, boolean> = {
				chat: true, file: true, shell: false, web: true,
				contacts: false, desktop: true, media: false, system: true
			};
			for (const key of Object.keys(permissions)) {
				permissions[key] = defaults[key] ?? false;
			}
			permissions = { ...permissions };
			autoApproveRead = true;
			autoApproveWrite = false;
			autoApproveBash = false;
			saveAll();
		}
	}

	function handleApprovalToggle() {
		saveAll();
	}

	function handleTermsCancel() {
		showTermsModal = false;
		termsAccepted = false;
		confirmText = '';
	}

	async function handleTermsConfirm() {
		if (!canConfirmTerms) return;

		try {
			await api.acceptTerms();
			autonomousMode = true;
			for (const key of Object.keys(permissions)) {
				permissions[key] = true;
			}
			permissions = { ...permissions };
			autoApproveRead = true;
			autoApproveWrite = true;
			autoApproveBash = true;
			await saveAll();
		} catch (err: any) {
			console.error('Failed to accept terms:', err);
		} finally {
			showTermsModal = false;
			termsAccepted = false;
			confirmText = '';
		}
	}
</script>

<!-- Header -->
<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsPermissions.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsPermissions.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsPermissions.loadingPermissions')}</span>
	</div>
{:else}
	<div class="space-y-6">
		<!-- Autonomous Mode -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsPermissions.autonomousMode')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-start justify-between">
					<div class="flex-1 pr-4">
						<p class="text-base font-medium text-base-content flex items-center gap-2">
							<AlertTriangle class="w-4 h-4 text-warning" />
							{$t('settingsPermissions.fullAutonomous')}
						</p>
						<p class="text-base text-base-content/80 mt-1">
							{$t('settingsPermissions.fullAutonomousDesc')}
						</p>
					</div>
					<input
						type="checkbox"
						class="toggle toggle-primary mt-0.5"
						bind:checked={autonomousMode}
						onchange={handleAutonomousChange}
					/>
				</div>

				{#if autonomousMode}
					<div class="mt-4 rounded-xl bg-warning/10 border border-warning/20 px-4 py-3">
						<p class="text-base text-warning font-medium">{$t('settingsPermissions.autonomousActive')}</p>
						<p class="text-base text-base-content/80 mt-0.5">
							{$t('settingsPermissions.autonomousActiveDesc')}
						</p>
					</div>
				{/if}
			</div>
		</section>

		<!-- Capabilities -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsPermissions.capabilities')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
				{#each capabilityGroups as cap}
					<button
						type="button"
						class="permissions-capability-row w-full flex items-center gap-4 px-5 py-4 text-left transition-colors
							{cap.alwaysOn || autonomousMode ? 'opacity-70 cursor-default' : 'hover:bg-base-content/5 cursor-pointer'}"
						onclick={() => togglePermission(cap.key)}
						disabled={cap.alwaysOn || autonomousMode}
					>
						<div class="w-9 h-9 rounded-xl flex items-center justify-center shrink-0
							{permissions[cap.key] ? 'bg-primary/10' : 'bg-base-content/5'}">
							<cap.icon class="w-4.5 h-4.5 {permissions[cap.key] ? 'text-primary' : 'text-base-content/80'}" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="text-base font-medium text-base-content">{$t(cap.labelKey)}</span>
								{#if cap.alwaysOn}
									<span class="text-sm font-medium text-base-content/80 bg-base-content/5 px-1.5 py-0.5 rounded">{$t('common.required')}</span>
								{/if}
								{#if autonomousMode && !cap.alwaysOn}
									<span class="text-sm font-medium text-warning bg-warning/10 px-1.5 py-0.5 rounded">{$t('settingsPermissions.auto')}</span>
								{/if}
							</div>
							<p class="text-base text-base-content/80 mt-0.5">{$t(cap.descriptionKey)}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary toggle-sm"
							checked={permissions[cap.key]}
							disabled={cap.alwaysOn || autonomousMode}
							onclick={(e: MouseEvent) => e.stopPropagation()}
							onchange={() => togglePermission(cap.key)}
						/>
					</button>
				{/each}
			</div>
		</section>

		<!-- Tool Approval Policy (only when NOT autonomous) -->
		{#if !autonomousMode}
			<section>
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsPermissions.toolApprovalPolicy')}</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-0 divide-y divide-base-content/10">
					<div class="flex items-center justify-between py-3 first:pt-0 last:pb-0">
						<div>
							<p class="text-base font-medium text-base-content">{$t('settingsPermissions.autoFileReads')}</p>
							<p class="text-base text-base-content/80 mt-0.5">{$t('settingsPermissions.autoFileReadsDesc')}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary toggle-sm"
							bind:checked={autoApproveRead}
							onchange={handleApprovalToggle}
						/>
					</div>

					<div class="flex items-center justify-between py-3 first:pt-0 last:pb-0">
						<div>
							<p class="text-base font-medium text-base-content">{$t('settingsPermissions.autoFileWrites')}</p>
							<p class="text-base text-base-content/80 mt-0.5">{$t('settingsPermissions.autoFileWritesDesc')}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary toggle-sm"
							bind:checked={autoApproveWrite}
							onchange={handleApprovalToggle}
						/>
					</div>

					<div class="flex items-center justify-between py-3 first:pt-0 last:pb-0">
						<div>
							<p class="text-base font-medium text-base-content">{$t('settingsPermissions.autoShell')}</p>
							<p class="text-base text-base-content/80 mt-0.5">{$t('settingsPermissions.autoShellDesc')}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary toggle-sm"
							bind:checked={autoApproveBash}
							onchange={handleApprovalToggle}
						/>
					</div>
				</div>
			</section>
		{/if}

		<!-- Save Error -->
		{#if saveError}
			<div class="rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-base text-error">
				{saveError}
			</div>
		{/if}
	</div>
{/if}

<!-- Terms Acceptance Modal -->
<Modal
	bind:show={showTermsModal}
	title={$t('settingsPermissions.enableTitle')}
	size="lg"
	closeOnBackdrop={false}
	showCloseButton={false}
	onclose={handleTermsCancel}
>
	<div class="space-y-4">
		<div class="flex items-start gap-3">
			<AlertTriangle class="w-6 h-6 text-warning shrink-0 mt-0.5" />
			<p class="text-base text-base-content">
				{$t('settingsPermissions.enableDescription')}
			</p>
		</div>

		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<p class="text-base font-semibold text-error mb-2">{$t('settingsPermissions.risks')}</p>
			<ul class="text-base text-base-content/80 space-y-1 list-disc list-inside">
				<li>{$t('settingsPermissions.risk1')}</li>
				<li>{$t('settingsPermissions.risk2')}</li>
				<li>{$t('settingsPermissions.risk3')}</li>
				<li>{$t('settingsPermissions.risk4')}</li>
			</ul>
		</div>

		<div class="rounded-xl bg-base-200 p-4 max-h-40 overflow-y-auto">
			<p class="text-base text-base-content/80 leading-relaxed">
				{$t('settingsPermissions.disclaimer')}
			</p>
		</div>

		<label class="flex items-center gap-3 cursor-pointer">
			<input type="checkbox" class="checkbox checkbox-warning" bind:checked={termsAccepted} />
			<span class="text-base font-medium text-base-content">
				{$t('settingsPermissions.acceptRisks')}
			</span>
		</label>

		<div>
			<label class="block text-base font-medium text-base-content mb-1" for="confirm-enable">
				{$t('settingsPermissions.typeEnable')}
			</label>
			<input
				id="confirm-enable"
				type="text"
				class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
				placeholder={$t('settingsPermissions.typeEnable')}
				bind:value={confirmText}
				onkeydown={(e) => {
					if (e.key === 'Enter' && canConfirmTerms) handleTermsConfirm();
				}}
			/>
		</div>
	</div>

	{#snippet footer()}
		<div class="flex justify-end gap-2 w-full">
			<button
				type="button"
				class="h-9 px-4 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
				onclick={handleTermsCancel}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="button"
				class="h-9 px-4 rounded-full bg-error text-white text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
				onclick={handleTermsConfirm}
				disabled={!canConfirmTerms}
			>
				{$t('settingsPermissions.enableTitle')}
			</button>
		</div>
	{/snippet}
</Modal>
