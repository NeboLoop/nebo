<script lang="ts">
	import { onMount } from 'svelte';
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
			label: 'Chat & Memory',
			description: 'Core conversations, memory storage, and scheduled tasks. Required for basic operation.',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			label: 'File System',
			description: 'Read, write, edit, search, and browse files on your computer.',
			icon: FileText
		},
		{
			key: 'shell',
			label: 'Shell & Terminal',
			description: 'Execute commands, manage background processes, and run scripts.',
			icon: Terminal
		},
		{
			key: 'web',
			label: 'Web Browsing',
			description: 'Fetch web pages, search the internet, and automate browser interactions.',
			icon: Globe
		},
		{
			key: 'contacts',
			label: 'Contacts & Calendar',
			description: 'Access your contacts, calendar events, reminders, and mail.',
			icon: Users
		},
		{
			key: 'desktop',
			label: 'Desktop Control',
			description: 'Manage windows, accessibility features, and clipboard.',
			icon: Monitor
		},
		{
			key: 'media',
			label: 'Media & Capture',
			description: 'Screenshots, image analysis, music playback, and text-to-speech.',
			icon: Camera
		},
		{
			key: 'system',
			label: 'System',
			description: 'Spotlight, keychain, Siri shortcuts, system info, and notifications.',
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
			saveError = err?.message || 'Failed to save settings';
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
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Permissions</h2>
	<p class="text-sm text-base-content/70">Control what capabilities your agent has access to and how it handles approvals.</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-sm text-base-content/70">Loading permissions...</span>
	</div>
{:else}
	<div class="space-y-6">
		<!-- Autonomous Mode -->
		<section>
			<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Autonomous Mode</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-start justify-between">
					<div class="flex-1 pr-4">
						<p class="text-sm font-medium text-base-content flex items-center gap-2">
							<AlertTriangle class="w-4 h-4 text-warning" />
							100% Autonomous
						</p>
						<p class="text-sm text-base-content/70 mt-1">
							The agent will execute ALL tools without asking for permission — shell commands, file modifications, and network requests.
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
						<p class="text-sm text-warning font-medium">Autonomous Mode is active</p>
						<p class="text-sm text-base-content/70 mt-0.5">
							All approval prompts are bypassed. Make sure you trust the prompts you're sending.
						</p>
					</div>
				{/if}
			</div>
		</section>

		<!-- Capabilities -->
		<section>
			<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Capabilities</h3>
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
							<cap.icon class="w-4.5 h-4.5 {permissions[cap.key] ? 'text-primary' : 'text-base-content/50'}" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-base-content">{cap.label}</span>
								{#if cap.alwaysOn}
									<span class="text-[11px] font-medium text-base-content/50 bg-base-content/5 px-1.5 py-0.5 rounded">Required</span>
								{/if}
								{#if autonomousMode && !cap.alwaysOn}
									<span class="text-[11px] font-medium text-warning bg-warning/10 px-1.5 py-0.5 rounded">Auto</span>
								{/if}
							</div>
							<p class="text-sm text-base-content/70 mt-0.5">{cap.description}</p>
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
				<h3 class="text-sm font-semibold text-base-content/70 uppercase tracking-wider mb-3">Tool Approval Policy</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-0 divide-y divide-base-content/10">
					<div class="flex items-center justify-between py-3 first:pt-0 last:pb-0">
						<div>
							<p class="text-sm font-medium text-base-content">Auto-approve File Reads</p>
							<p class="text-sm text-base-content/70 mt-0.5">Allow reading files without prompting</p>
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
							<p class="text-sm font-medium text-base-content">Auto-approve File Writes</p>
							<p class="text-sm text-base-content/70 mt-0.5">Allow creating and editing files without prompting</p>
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
							<p class="text-sm font-medium text-base-content">Auto-approve Shell Commands</p>
							<p class="text-sm text-base-content/70 mt-0.5">Allow executing bash commands without prompting</p>
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
			<div class="rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-sm text-error">
				{saveError}
			</div>
		{/if}
	</div>
{/if}

<!-- Terms Acceptance Modal -->
<Modal
	bind:show={showTermsModal}
	title="Enable Autonomous Mode"
	size="lg"
	closeOnBackdrop={false}
	showCloseButton={false}
	onclose={handleTermsCancel}
>
	<div class="space-y-4">
		<div class="flex items-start gap-3">
			<AlertTriangle class="w-6 h-6 text-warning shrink-0 mt-0.5" />
			<p class="text-sm text-base-content">
				This will allow your agent to execute all tools — including shell commands, file
				modifications, and network requests — <strong>without asking for permission</strong>.
			</p>
		</div>

		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<p class="text-sm font-semibold text-error mb-2">Risks include:</p>
			<ul class="text-sm text-base-content/80 space-y-1 list-disc list-inside">
				<li>The agent may modify or delete files on your system</li>
				<li>The agent may execute arbitrary shell commands</li>
				<li>The agent may make network requests and access external services</li>
				<li>You are solely responsible for any actions taken by the agent</li>
			</ul>
		</div>

		<div class="rounded-xl bg-base-200 p-4 max-h-40 overflow-y-auto">
			<p class="text-sm text-base-content/70 leading-relaxed">
				By enabling Autonomous Mode, you acknowledge and agree that: (1) You assume all risk
				and responsibility for any actions performed by the agent while operating in
				autonomous mode. (2) Nebo Labs, Inc. and its affiliates, officers, employees, and
				contributors shall not be liable for any damages, data loss, security incidents, or
				unintended consequences arising from autonomous agent operation. (3) You have reviewed
				and understand the full scope of capabilities enabled by this mode, including
				unrestricted file system access, shell command execution, and network requests. (4)
				You agree to indemnify and hold harmless Nebo Labs, Inc. from any claims, losses, or
				damages resulting from your use of Autonomous Mode.
			</p>
		</div>

		<label class="flex items-center gap-3 cursor-pointer">
			<input type="checkbox" class="checkbox checkbox-warning" bind:checked={termsAccepted} />
			<span class="text-sm font-medium text-base-content">
				I understand the risks and accept full responsibility
			</span>
		</label>

		<div>
			<label class="block text-sm font-medium text-base-content mb-1" for="confirm-enable">
				Type <code class="bg-base-200 px-1.5 py-0.5 rounded text-error font-bold">ENABLE</code> to confirm
			</label>
			<input
				id="confirm-enable"
				type="text"
				class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-sm focus:outline-none focus:border-primary/50 transition-colors"
				placeholder="Type ENABLE to confirm"
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
				class="h-9 px-4 rounded-full border border-base-content/10 text-sm font-medium hover:bg-base-content/5 transition-colors"
				onclick={handleTermsCancel}
			>
				Cancel
			</button>
			<button
				type="button"
				class="h-9 px-4 rounded-full bg-error text-white text-sm font-bold hover:brightness-110 transition-all disabled:opacity-30"
				onclick={handleTermsConfirm}
				disabled={!canConfirmTerms}
			>
				Enable Autonomous Mode
			</button>
		</div>
	{/snippet}
</Modal>
