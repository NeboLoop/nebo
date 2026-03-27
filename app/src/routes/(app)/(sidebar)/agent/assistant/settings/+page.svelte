<script lang="ts">
	import { t } from 'svelte-i18n';
</script>

<svelte:head>
	<title>Nebo - {$t('agent.assistant')} - {$t('agent.settingsTab')}</title>
</svelte:head>

<div class="flex-1 overflow-y-auto">
	<div class="max-w-3xl mx-auto px-6 py-8">
		<h2 class="font-display text-lg font-bold mb-6">{$t('settingsAssistant.title')}</h2>

		<div class="flex flex-col items-center py-12 text-center">
			<svg class="w-10 h-10 text-base-content/20 mb-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
				<path d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
				<circle cx="12" cy="12" r="3" />
			</svg>
			<p class="text-sm text-base-content/50">{$t('settingsAssistant.description')}</p>
			<p class="text-xs text-base-content/40 mt-1">Configure these in <a href="/settings/personality" class="link link-primary">{$t('settingsAssistant.personalityLink')}</a> and <a href="/settings/providers" class="link link-primary">{$t('settingsAssistant.modelsLink')}</a>.</p>
		</div>
	</div>
</div>
