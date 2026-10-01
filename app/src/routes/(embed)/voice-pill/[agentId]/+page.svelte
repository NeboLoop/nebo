<script lang="ts">
	/**
	 * An app window's voice pill: its own small window pinned to the app
	 * window's corner, outside the app's page, so the employee rebuilding and
	 * reloading the page never touches the call. It drives the main window's
	 * one call over Tauri events (see `$lib/apps/voiceBridge`).
	 */
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { hasVoiceCloudConsent, grantVoiceCloudConsent } from '$lib/stores/voiceSession';
	import {
		VOICE_COMMAND,
		VOICE_STATE,
		type AppVoiceAction,
		type AppVoiceState
	} from '$lib/apps/voiceBridge';
	import Mic from 'lucide-svelte/icons/mic';
	import MicOff from 'lucide-svelte/icons/mic-off';
	import AudioLines from 'lucide-svelte/icons/audio-lines';
	import PhoneOff from 'lucide-svelte/icons/phone-off';

	const agentId = $page.params.agentId ?? '';
	const name = $page.url.searchParams.get('name') || '';

	let call = $state<AppVoiceState>({ status: 'idle', agentId: null, muted: false });
	let asking = $state(false);
	let send: (action: AppVoiceAction) => void = () => {};

	const mine = $derived(call.status !== 'idle' && call.agentId === agentId);
	const elsewhere = $derived(call.status !== 'idle' && call.status !== 'error' && call.agentId !== agentId);
	const live = $derived(mine && ['listening', 'processing', 'speaking'].includes(call.status));

	const label = $derived(
		call.status === 'connecting' ? $t('settingsPlugins.connecting')
		: call.status === 'reconnecting' ? $t('voice.reconnecting')
		: call.status === 'listening' ? (call.muted ? $t('chatInput.muted') : $t('chatInput.listening'))
		: call.status === 'processing' ? $t('chatInput.thinking')
		: call.status === 'speaking' ? $t('voice.isSpeaking', { values: { name } })
		: call.status === 'error' ? $t('voice.callEnded')
		: ''
	);

	function start() {
		if (!hasVoiceCloudConsent()) {
			asking = true;
			return;
		}
		send('start');
	}

	function consent() {
		grantVoiceCloudConsent();
		asking = false;
		send('start');
	}

	onMount(() => {
		let stop = () => {};
		void (async () => {
			const { emit, listen } = await import('@tauri-apps/api/event');
			send = (action) => void emit(VOICE_COMMAND, { action, agentId });
			stop = await listen<AppVoiceState>(VOICE_STATE, (e) => {
				if (e.payload) call = e.payload;
			});
			send('hello');
		})();
		return () => stop();
	});
</script>

<div class="h-dvh w-full flex items-center gap-2 px-3 bg-base-100 text-base-content select-none overflow-hidden">
	{#if asking}
		<span class="flex-1 min-w-0 truncate text-xs text-base-content/70">{$t('voice.consentTitle')}</span>
		<button class="btn btn-ghost btn-xs" onclick={() => (asking = false)}>{$t('voice.consentDecline')}</button>
		<button class="btn btn-primary btn-xs" onclick={consent}>{$t('voice.consentAccept')}</button>
	{:else if mine}
		<span class="grid place-items-center w-5 h-5 shrink-0">
			{#if call.status === 'connecting' || call.status === 'reconnecting'}
				<span class="loading loading-spinner loading-xs text-base-content/50"></span>
			{:else if call.status === 'speaking'}
				<AudioLines class="w-4 h-4 text-primary" />
			{:else if call.status === 'processing'}
				<span class="loading loading-dots loading-xs text-primary"></span>
			{:else if call.status === 'error'}
				<span class="text-error font-semibold">!</span>
			{:else if call.muted}
				<MicOff class="w-4 h-4 text-base-content/50" />
			{:else}
				<Mic class="w-4 h-4 text-primary" />
			{/if}
		</span>
		<span class="flex-1 min-w-0 truncate text-sm font-medium">{label}</span>
		{#if call.status === 'error'}
			<button class="btn btn-ghost btn-xs" onclick={start}>{$t('voice.tryAgain')}</button>
		{:else}
			<button
				class="btn btn-ghost btn-circle btn-sm"
				title={call.muted ? $t('chatInput.unmute') : $t('chatInput.mute')}
				aria-label={call.muted ? $t('chatInput.unmute') : $t('chatInput.mute')}
				disabled={!live}
				onclick={() => send('mute')}
			>
				{#if call.muted}<MicOff class="w-4 h-4" />{:else}<Mic class="w-4 h-4" />{/if}
			</button>
		{/if}
		<button
			class="btn btn-error btn-circle btn-sm"
			title={$t('chatInput.endConversation')}
			aria-label={$t('chatInput.endConversation')}
			onclick={() => send('end')}
		>
			<PhoneOff class="w-4 h-4" />
		</button>
	{:else if elsewhere}
		<MicOff class="w-4 h-4 shrink-0 text-base-content/40" />
		<span class="flex-1 min-w-0 truncate text-xs text-base-content/60">{$t('voice.onAnotherCall')}</span>
	{:else}
		<button class="btn btn-ghost btn-sm flex-1 justify-start gap-2 min-w-0" onclick={start}>
			<Mic class="w-4 h-4 shrink-0 text-primary" />
			<span class="truncate">{$t('voice.talkTo', { values: { name } })}</span>
		</button>
	{/if}
</div>
