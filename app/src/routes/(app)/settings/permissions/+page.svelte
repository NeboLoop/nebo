<script lang="ts">
	import { onMount } from 'svelte';
	import {
		Shield,
		FileText,
		Terminal,
		Globe,
		Users,
		Monitor,
		Camera,
		Cpu,
		MessageCircle,
		Zap,
		AlertTriangle
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Card from '$lib/components/ui/Card.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import Button from '$lib/components/ui/Button.svelte';

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
			description:
				'Core conversations, memory storage, and scheduled tasks. This is required for basic operation.',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			label: 'File System',
			description:
				'Read, write, edit, search, and browse files on your computer. Required for most tasks.',
			icon: FileText
		},
		{
			key: 'shell',
			label: 'Shell & Terminal',
			description:
				'Execute commands, manage background processes, and run scripts in your terminal.',
			icon: Terminal
		},
		{
			key: 'web',
			label: 'Web Browsing',
			description:
				'Fetch web pages, search the internet, and automate browser interactions.',
			icon: Globe
		},
		{
			key: 'contacts',
			label: 'Contacts & Calendar',
			description:
				'Access your contacts, calendar events, reminders, and mail application.',
			icon: Users
		},
		{
			key: 'desktop',
			label: 'Desktop Control',
			description:
				'Manage windows, use accessibility features, and access the clipboard.',
			icon: Monitor
		},
		{
			key: 'media',
			label: 'Media & Capture',
			description:
				'Take screenshots, analyze images, control music playback, and use text-to-speech.',
			icon: Camera
		},
		{
			key: 'system',
			label: 'System',
			description:
				'Spotlight search, keychain access, Siri shortcuts, system information, and notifications.',
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
			// Just toggled ON — immediately revert and show terms modal instead
			autonomousMode = false;
			showTermsModal = true;
		} else {
			// Turning OFF — reset to sensible defaults
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

<div class="space-y-6">
	<!-- Header -->
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Permissions</h2>
		<p class="text-sm text-base-content/60">
			Control what capabilities your agent has access to and how it handles approvals.
		</p>
	</div>

	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading permissions...</p>
			</div>
		</Card>
	{:else}
		<!-- Autonomous Mode -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-error/10 flex items-center justify-center">
					<Zap class="w-5 h-5 text-error" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Autonomous Mode</h2>
					<p class="text-sm text-base-content/60">Run the agent without approval prompts</p>
				</div>
			</div>

			<div class="flex items-start justify-between py-3">
				<div class="flex-1 pr-4">
					<p class="text-sm font-medium text-base-content flex items-center gap-2">
						<AlertTriangle class="w-4 h-4 text-warning" />
						100% Autonomous Mode
					</p>
					<p class="text-xs text-base-content/60 mt-1">
						The agent will execute ALL tools without asking for permission. This includes
						shell commands, file modifications, and network requests.
						<strong class="text-error">Use with extreme caution.</strong>
					</p>
				</div>
				<input
					type="checkbox"
					class="toggle toggle-primary"
					bind:checked={autonomousMode}
					onchange={handleAutonomousChange}
				/>
			</div>

			{#if autonomousMode}
				<Alert type="warning" title="Autonomous Mode Enabled">
					The agent will bypass all approval prompts and execute tools automatically. Make
					sure you trust the prompts you're sending and have backups of important data.
				</Alert>
			{/if}
		</Card>

		<!-- Capability Permissions -->
		<div class="space-y-3">
			{#each capabilityGroups as cap}
				<div
					class="p-4 rounded-xl border transition-all
						{permissions[cap.key]
						? 'border-primary/30 bg-primary/5'
						: 'border-base-300'}
						{cap.alwaysOn || autonomousMode
						? 'opacity-80'
						: 'cursor-pointer hover:border-base-content/20'}"
					role="button"
					tabindex={cap.alwaysOn || autonomousMode ? -1 : 0}
					onclick={() => togglePermission(cap.key)}
					onkeydown={(e) => {
						if (e.key === 'Enter' || e.key === ' ') {
							e.preventDefault();
							togglePermission(cap.key);
						}
					}}
				>
					<div class="flex items-start gap-4">
						<div
							class="p-2 rounded-lg {permissions[cap.key] ? 'bg-primary/20' : 'bg-base-200'}"
						>
							<cap.icon
								class="w-5 h-5 {permissions[cap.key]
									? 'text-primary'
									: 'text-base-content/50'}"
							/>
						</div>
						<div class="flex-1">
							<div class="flex items-center gap-2 mb-1">
								<span class="font-semibold">{cap.label}</span>
								{#if cap.alwaysOn}
									<span class="badge badge-neutral badge-sm">Required</span>
								{/if}
								{#if autonomousMode && !cap.alwaysOn}
									<span class="badge badge-warning badge-sm">Auto</span>
								{/if}
							</div>
							<p class="text-sm text-base-content/60">{cap.description}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary"
							checked={permissions[cap.key]}
							disabled={cap.alwaysOn || autonomousMode}
							onclick={(e: MouseEvent) => e.stopPropagation()}
							onchange={() => togglePermission(cap.key)}
						/>
					</div>
				</div>
			{/each}
		</div>

		<!-- Tool Approval Policy (only when NOT autonomous) -->
		{#if !autonomousMode}
			<Card>
				<div class="flex items-center gap-3 mb-6">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
						<Shield class="w-5 h-5 text-primary" />
					</div>
					<div>
						<h2 class="text-lg font-semibold text-base-content">Tool Approval Policy</h2>
						<p class="text-sm text-base-content/60">
							Configure which tools auto-approve without prompting
						</p>
					</div>
				</div>

				<div class="space-y-4">
					<div class="flex items-center justify-between py-3 border-b border-base-content/10">
						<div>
							<p class="text-sm font-medium text-base-content">Auto-approve File Reads</p>
							<p class="text-xs text-base-content/60">
								Allow reading files without prompting
							</p>
						</div>
						<Toggle bind:checked={autoApproveRead} onchange={handleApprovalToggle} />
					</div>

					<div class="flex items-center justify-between py-3 border-b border-base-content/10">
						<div>
							<p class="text-sm font-medium text-base-content">
								Auto-approve File Writes
							</p>
							<p class="text-xs text-base-content/60">
								Allow creating/editing files without prompting
							</p>
						</div>
						<Toggle bind:checked={autoApproveWrite} onchange={handleApprovalToggle} />
					</div>

					<div class="flex items-center justify-between py-3">
						<div>
							<p class="text-sm font-medium text-base-content">
								Auto-approve Shell Commands
							</p>
							<p class="text-xs text-base-content/60">
								Allow executing bash commands without prompting
							</p>
						</div>
						<Toggle bind:checked={autoApproveBash} onchange={handleApprovalToggle} />
					</div>
				</div>
			</Card>
		{/if}

		<!-- Save Feedback -->
		{#if saveError}
			<Alert type="error">
				{saveError}
			</Alert>
		{/if}
	{/if}
</div>

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

		<div class="bg-error/10 border border-error/20 rounded-lg p-4">
			<p class="text-sm font-semibold text-error mb-2">Risks include:</p>
			<ul class="text-sm text-base-content/80 space-y-1 list-disc list-inside">
				<li>The agent may modify or delete files on your system</li>
				<li>The agent may execute arbitrary shell commands</li>
				<li>The agent may make network requests and access external services</li>
				<li>You are solely responsible for any actions taken by the agent</li>
			</ul>
		</div>

		<div class="bg-base-200 rounded-lg p-4 max-h-40 overflow-y-auto">
			<p class="text-xs text-base-content/70 leading-relaxed">
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
				Type <code class="bg-base-200 px-1.5 py-0.5 rounded text-error font-bold">ENABLE</code> to
				confirm
			</label>
			<input
				id="confirm-enable"
				type="text"
				class="input input-bordered w-full text-sm"
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
			<Button type="ghost" onclick={handleTermsCancel}>Cancel</Button>
			<Button type="danger" onclick={handleTermsConfirm} disabled={!canConfirmTerms}>
				Enable Autonomous Mode
			</Button>
		</div>
	{/snippet}
</Modal>
