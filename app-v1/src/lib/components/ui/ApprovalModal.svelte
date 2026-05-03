<script lang="ts">
	import { t } from 'svelte-i18n';
	import { AlertTriangle, Terminal, Check, X, CheckCheck } from 'lucide-svelte';

	interface ApprovalRequest {
		requestId: string;
		tool: string;
		input: Record<string, unknown>;
	}

	let {
		request,
		onApprove,
		onApproveAlways,
		onDeny
	}: {
		request: ApprovalRequest | null;
		onApprove: (requestId: string) => void;
		onApproveAlways: (requestId: string) => void;
		onDeny: (requestId: string) => void;
	} = $props();

	function getInputDisplay(input: Record<string, unknown>): string {
		if (input.command) {
			return input.command as string;
		}
		if (input.path) {
			return input.path as string;
		}
		return JSON.stringify(input, null, 2);
	}
</script>

{#if request}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
		<div class="bg-base-200 rounded-2xl shadow-xl max-w-lg w-full mx-4 overflow-hidden border border-base-300">
			<div class="px-6 py-4 bg-warning/10 border-b border-warning/20 flex items-center gap-3">
				<div class="p-2 rounded-full bg-warning/20">
					<AlertTriangle class="w-5 h-5 text-warning" />
				</div>
				<div>
					<h3 class="font-semibold text-base-content">{$t('approval.title')}</h3>
					<p class="text-base text-base-content/90">{$t('approval.description')}</p>
				</div>
			</div>

			<div class="p-6 space-y-4">
				<div class="flex items-center gap-2">
					<Terminal class="w-4 h-4 text-secondary" />
					<span class="font-mono text-base text-secondary">{request.tool}</span>
				</div>

				<div class="bg-base-300 rounded-lg p-4 overflow-auto max-h-48">
					<pre class="text-base font-mono text-base-content whitespace-pre-wrap break-all">{getInputDisplay(request.input)}</pre>
				</div>

				<div class="flex gap-3 pt-2">
					<button
						type="button"
						onclick={() => onDeny(request.requestId)}
						class="btn btn-outline gap-2"
					>
						<X class="w-4 h-4" />
						{$t('approval.deny')}
					</button>
					<button
						type="button"
						onclick={() => onApprove(request.requestId)}
						class="btn btn-primary flex-1 gap-2"
					>
						<Check class="w-4 h-4" />
						{$t('approval.once')}
					</button>
					<button
						type="button"
						onclick={() => onApproveAlways(request.requestId)}
						class="btn btn-success flex-1 gap-2"
					>
						<CheckCheck class="w-4 h-4" />
						{$t('approval.always')}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
