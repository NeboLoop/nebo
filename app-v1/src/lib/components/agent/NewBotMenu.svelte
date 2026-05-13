<script lang="ts">
	import { goto } from '$app/navigation';
	import { createBlankAgent } from '$lib/api/nebo';
	import { t } from 'svelte-i18n';

	let {
		onClose = () => {},
	}: {
		onClose?: () => void;
	} = $props();

	let creating = $state(false);

	function browseMarketplace() {
		onClose();
		goto('/marketplace?type=agent');
	}

	async function createNew() {
		if (creating) return;
		creating = true;
		onClose();
		try {
			const res = await createBlankAgent();
			goto(`/agent/persona/${res.agent.id}/chat`);
		} catch (e) {
			console.error('Failed to create blank agent:', e);
			creating = false;
		}
	}
</script>

<!-- Invisible full-screen backdrop to catch outside clicks -->
<div class="new-bot-menu-backdrop" onclick={onClose} onkeydown={() => {}} role="presentation"></div>

<div class="new-bot-menu">
	<button class="new-bot-menu-item" onclick={browseMarketplace}>
		<div class="new-bot-menu-item-icon">
			<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<circle cx="11" cy="11" r="8" />
				<line x1="21" y1="21" x2="16.65" y2="16.65" />
			</svg>
		</div>
		<div class="flex flex-col">
			<span class="font-medium text-base-content">{$t('newBot.browseMarketplace')}</span>
			<span class="text-sm text-base-content/60">{$t('newBot.findPrebuilt')}</span>
		</div>
	</button>
	<button class="new-bot-menu-item" onclick={createNew} disabled={creating}>
		<div class="new-bot-menu-item-icon">
			<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
			</svg>
		</div>
		<div class="flex flex-col">
			<span class="font-medium text-base-content">{$t('newBot.newAgent')}</span>
			<span class="text-sm text-base-content/60">{$t('newBot.createBlank')}</span>
		</div>
	</button>
</div>
