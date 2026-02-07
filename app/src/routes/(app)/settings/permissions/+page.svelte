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
		Loader2,
		Save
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Button from '$lib/components/ui/Button.svelte';

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
	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let hasChanges = $state(false);

	// Track original state for dirty detection
	let originalPermissions = $state<Record<string, boolean>>({});

	const capabilityGroups = [
		{
			key: 'chat',
			label: 'Chat & Memory',
			description: 'Core conversations, memory storage, and scheduled tasks. This is required for basic operation.',
			icon: MessageCircle,
			alwaysOn: true
		},
		{
			key: 'file',
			label: 'File System',
			description: 'Read, write, edit, search, and browse files on your computer. Required for coding assistance.',
			icon: FileText
		},
		{
			key: 'shell',
			label: 'Shell & Terminal',
			description: 'Execute commands, manage background processes, and run scripts in your terminal.',
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
			description: 'Access your contacts, calendar events, reminders, and mail application.',
			icon: Users
		},
		{
			key: 'desktop',
			label: 'Desktop Control',
			description: 'Manage windows, use accessibility features, and access the clipboard.',
			icon: Monitor
		},
		{
			key: 'media',
			label: 'Media & Capture',
			description: 'Take screenshots, analyze images, control music playback, and use text-to-speech.',
			icon: Camera
		},
		{
			key: 'system',
			label: 'System',
			description: 'Spotlight search, keychain access, Siri shortcuts, system information, and notifications.',
			icon: Cpu
		}
	];

	onMount(async () => {
		try {
			const response = await api.getToolPermissions();
			if (response.permissions && Object.keys(response.permissions).length > 0) {
				permissions = { ...permissions, ...response.permissions };
			}
		} catch {
			// Use defaults on error
		} finally {
			originalPermissions = { ...permissions };
			isLoading = false;
		}
	});

	function togglePermission(key: string) {
		if (key === 'chat') return;
		permissions = { ...permissions, [key]: !permissions[key] };
		hasChanges = JSON.stringify(permissions) !== JSON.stringify(originalPermissions);
	}

	async function savePermissions() {
		isSaving = true;
		saveMessage = '';

		try {
			await api.updateToolPermissions({ permissions });
			originalPermissions = { ...permissions };
			hasChanges = false;
			saveMessage = 'Permissions saved. Changes take effect on the next agent restart.';
			setTimeout(() => {
				saveMessage = '';
			}, 4000);
		} catch (err: any) {
			saveMessage = err?.message || 'Failed to save permissions';
		} finally {
			isSaving = false;
		}
	}
</script>

<div class="space-y-6">
	<div>
		<div class="flex items-center gap-3 mb-1">
			<Shield class="w-5 h-5 text-primary" />
			<h2 class="text-xl font-bold">Agent Permissions</h2>
		</div>
		<p class="text-sm text-base-content/60">
			Control what capabilities your agent has access to. Only enabled capabilities will be registered as tools.
		</p>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<Loader2 class="w-6 h-6 animate-spin text-primary" />
			<span class="ml-2 text-base-content/60">Loading permissions...</span>
		</div>
	{:else}
		<div class="space-y-3">
			{#each capabilityGroups as cap}
				<div
					class="p-4 rounded-xl border transition-all
						{permissions[cap.key]
							? 'border-primary/30 bg-primary/5'
							: 'border-base-300'}
						{cap.alwaysOn ? 'opacity-80' : 'cursor-pointer hover:border-base-content/20'}"
					role="button"
					tabindex={cap.alwaysOn ? -1 : 0}
					onclick={() => togglePermission(cap.key)}
					onkeydown={(e) => {
						if (e.key === 'Enter' || e.key === ' ') {
							e.preventDefault();
							togglePermission(cap.key);
						}
					}}
				>
					<div class="flex items-start gap-4">
						<div class="p-2 rounded-lg {permissions[cap.key] ? 'bg-primary/20' : 'bg-base-200'}">
							<cap.icon class="w-5 h-5 {permissions[cap.key] ? 'text-primary' : 'text-base-content/50'}" />
						</div>
						<div class="flex-1">
							<div class="flex items-center gap-2 mb-1">
								<span class="font-semibold">{cap.label}</span>
								{#if cap.alwaysOn}
									<span class="badge badge-neutral badge-sm">Required</span>
								{/if}
							</div>
							<p class="text-sm text-base-content/60">{cap.description}</p>
						</div>
						<input
							type="checkbox"
							class="toggle toggle-primary"
							checked={permissions[cap.key]}
							disabled={cap.alwaysOn}
							onclick={(e: MouseEvent) => e.stopPropagation()}
							onchange={() => togglePermission(cap.key)}
						/>
					</div>
				</div>
			{/each}
		</div>

		{#if saveMessage}
			<div class="alert {saveMessage.includes('Failed') ? 'alert-error' : 'alert-success'}">
				<span>{saveMessage}</span>
			</div>
		{/if}

		<div class="flex justify-end">
			<Button
				type="primary"
				onclick={savePermissions}
				disabled={isSaving || !hasChanges}
			>
				{#if isSaving}
					<Loader2 class="w-4 h-4 mr-2 animate-spin" />
					Saving...
				{:else}
					<Save class="w-4 h-4 mr-2" />
					Save Changes
				{/if}
			</Button>
		</div>
	{/if}
</div>
