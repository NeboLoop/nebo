<script lang="ts">
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import Shield from 'lucide-svelte/icons/shield';
  import { approveAlways } from '$lib/stores/permissions.js';

  interface Props {
    show: boolean;
    agent?: string;
    actionType?: string;
    actionDetail?: string;
    actionKey?: string;
    onApprove?: () => void;
    onDeny?: () => void;
    onclose?: () => void;
  }

  let {
    show = $bindable(false),
    agent = 'Nebo',
    actionType = 'shell_command',
    actionDetail = 'rm -rf /tmp/cache',
    actionKey = '',
    onApprove,
    onDeny,
    onclose,
  }: Props = $props();

  const typeLabels: Record<string, string> = {
    shell_command: 'Run Shell Command',
    file_write: 'Write File',
    file_delete: 'Delete File',
    http_request: 'HTTP Request',
    browser_navigate: 'Open URL',
  };

  function handleDeny() {
    show = false;
    onDeny?.();
    onclose?.();
  }

  function handleApproveOnce() {
    show = false;
    onApprove?.();
    onclose?.();
  }

  function handleApproveAlways() {
    if (actionKey) approveAlways(actionKey);
    show = false;
    onApprove?.();
    onclose?.();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') handleDeny();
  }
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation" onclick={handleDeny} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); handleDeny(); } }}></div>
    <div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-warning/30 shadow-2xl overflow-hidden" role="presentation" onkeydown={handleKeydown}>
      <!-- Header -->
      <div class="flex items-center gap-3 px-5 py-4 bg-warning/5 border-b border-warning/20">
        <div class="w-9 h-9 rounded-full bg-warning/15 flex items-center justify-center">
          <AlertTriangle class="w-5 h-5 text-warning" />
        </div>
        <div>
          <h3 class="text-sm font-bold text-base-content">Action Requires Approval</h3>
          <p class="text-xs text-base-content/50">{agent} wants to perform an action</p>
        </div>
      </div>

      <!-- Body -->
      <div class="px-5 py-4">
        <div class="flex items-center gap-2 mb-3">
          <Shield class="w-4 h-4 text-base-content/40" />
          <span class="text-sm font-medium text-base-content/70">{typeLabels[actionType] || actionType}</span>
        </div>
        <div class="p-3 rounded-lg bg-base-200 border border-base-content/10 font-mono text-xs text-base-content break-all leading-relaxed">
          {actionDetail}
        </div>
      </div>

      <!-- Actions -->
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button
          onclick={handleDeny}
          class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
        >
          Deny
        </button>
        <button
          onclick={handleApproveOnce}
          class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
        >
          Approve Once
        </button>
        <button
          onclick={handleApproveAlways}
          class="px-4 py-2 rounded-lg bg-success text-success-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
        >
          Approve Always
        </button>
      </div>
    </div>
  </div>
{/if}
