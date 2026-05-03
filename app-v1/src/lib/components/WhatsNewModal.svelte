<!--
  What's New Modal
  Shown once after an update completes (version changed since last visit)
-->

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { CheckCircle, ExternalLink } from 'lucide-svelte';

	interface Props {
		show?: boolean;
		version: string;
		releaseUrl?: string;
		onclose?: () => void;
	}

	let {
		show = $bindable(false),
		version,
		releaseUrl,
		onclose
	}: Props = $props();

	function handleClose() {
		show = false;
		localStorage.setItem('nebo_last_seen_version', version);
		onclose?.();
	}
</script>

<Modal bind:show title={$t('whatsNew.title')} {onclose}>
	<div class="flex flex-col items-center text-center gap-4 py-4">
		<div class="w-16 h-16 rounded-full bg-success/15 flex items-center justify-center">
			<CheckCircle class="w-8 h-8 text-success" />
		</div>

		<div>
			<h3 class="text-lg font-bold text-base-content">{$t('whatsNew.heading')}</h3>
			<p class="text-base-content/70 mt-1">{$t('whatsNew.versionInfo', { values: { version } })}</p>
		</div>

		{#if releaseUrl}
			<a
				href={releaseUrl}
				target="_blank"
				rel="noopener noreferrer"
				class="inline-flex items-center gap-1.5 text-sm text-primary hover:text-primary/80 transition-colors"
			>
				{$t('whatsNew.viewReleaseNotes')}
				<ExternalLink class="w-3.5 h-3.5" />
			</a>
		{/if}
	</div>

	{#snippet footer()}
		<button class="btn btn-primary" onclick={handleClose}>
			{$t('common.gotIt')}
		</button>
	{/snippet}
</Modal>
