<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Globe, RefreshCw, X, Download } from 'lucide-svelte';

	let {
		show = $bindable(false),
		reason = 'not_connected',
		onRetry,
		onDismiss
	}: {
		show: boolean;
		reason: 'not_connected' | 'reconnecting';
		onRetry: () => void;
		onDismiss: () => void;
	} = $props();

	const STORE_URL = 'https://chromewebstore.google.com/detail/nebo-browser-relay/heaeiepdllbncnnlfniglgmbfmmemkcg';

	function handleInstall() {
		window.open(STORE_URL, '_blank');
		show = false;
	}
</script>

{#if show}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
		<div class="bg-base-200 rounded-2xl shadow-xl max-w-lg w-full mx-4 overflow-hidden border border-base-300">
			<div class="px-6 py-4 bg-info/10 border-b border-info/20 flex items-center gap-3">
				<div class="p-2 rounded-full bg-info/20">
					<Globe class="w-5 h-5 text-info" />
				</div>
				<div>
					<h3 class="font-semibold text-base-content">{$t('browserExtension.title')}</h3>
					<p class="text-base text-base-content/90">
						{reason === 'reconnecting'
							? $t('browserExtension.reconnecting')
							: $t('browserExtension.notConnected')}
					</p>
				</div>
			</div>

			<div class="p-6 space-y-4">
				<p class="text-base text-base-content/70">{$t('browserExtension.instructions')}</p>

				<div class="flex gap-3 pt-2">
					<button
						type="button"
						onclick={onDismiss}
						class="btn btn-outline gap-2"
					>
						<X class="w-4 h-4" />
						{$t('common.dismiss')}
					</button>
					<button
						type="button"
						onclick={onRetry}
						class="btn btn-ghost gap-2"
					>
						<RefreshCw class="w-4 h-4" />
						{$t('browserExtension.retry')}
					</button>
					<button
						type="button"
						onclick={handleInstall}
						class="btn btn-primary flex-1 gap-2"
					>
						<Download class="w-4 h-4" />
						{$t('browserExtension.install')}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
