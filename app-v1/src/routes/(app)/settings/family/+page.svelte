<script lang="ts">
	import { Users } from 'lucide-svelte';
	import { t } from 'svelte-i18n';
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsFamily.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsFamily.description')}</p>
</div>

<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-12 text-center">
	<Users class="w-10 h-10 mx-auto mb-3 text-base-content/60" />
	<h3 class="font-display font-bold text-base-content mb-1">{$t('common.comingSoon')}</h3>
	<p class="text-base text-base-content/80">{$t('settingsFamily.description')}</p>
</div>
