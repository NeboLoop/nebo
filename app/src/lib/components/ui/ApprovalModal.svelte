<script lang="ts">
	import { AlertTriangle, Terminal, Check, X } from 'lucide-svelte';

	interface ApprovalRequest {
		requestId: string;
		tool: string;
		input: Record<string, unknown>;
	}

	let {
		request,
		onApprove,
		onDeny
	}: {
		request: ApprovalRequest | null;
		onApprove: (requestId: string) => void;
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
					<h3 class="font-semibold text-base-content">Tool Approval Required</h3>
					<p class="text-sm text-base-content/60">The agent wants to run a tool</p>
				</div>
			</div>

			<div class="p-6 space-y-4">
				<div class="flex items-center gap-2">
					<Terminal class="w-4 h-4 text-secondary" />
					<span class="font-mono text-sm text-secondary">{request.tool}</span>
				</div>

				<div class="bg-base-300 rounded-lg p-4 overflow-x-auto">
					<pre class="text-sm font-mono text-base-content whitespace-pre-wrap break-all">{getInputDisplay(request.input)}</pre>
				</div>

				<div class="flex gap-3 pt-2">
					<button
						type="button"
						onclick={() => onDeny(request.requestId)}
						class="btn btn-outline flex-1 gap-2"
					>
						<X class="w-4 h-4" />
						Deny
					</button>
					<button
						type="button"
						onclick={() => onApprove(request.requestId)}
						class="btn btn-primary flex-1 gap-2"
					>
						<Check class="w-4 h-4" />
						Approve
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
