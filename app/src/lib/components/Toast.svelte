<script lang="ts">
  import { toasts, removeToast } from '$lib/stores/toast.js';
  import X from 'lucide-svelte/icons/x';
  import CheckCircle from 'lucide-svelte/icons/check-circle';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import AlertCircle from 'lucide-svelte/icons/alert-circle';
  import InfoIcon from 'lucide-svelte/icons/info';

  const iconMap = { success: CheckCircle, error: AlertCircle, warning: AlertTriangle, info: InfoIcon };
  const colorMap = {
    success: 'bg-success/10 border-success/30 text-success',
    error: 'bg-error/10 border-error/30 text-error',
    warning: 'bg-warning/10 border-warning/30 text-warning',
    info: 'bg-info/10 border-info/30 text-info',
  };
</script>

{#if $toasts.length > 0}
  <div class="fixed bottom-4 right-4 z-[100] flex flex-col gap-2 pointer-events-none">
    {#each $toasts as toast (toast.id)}
      {@const Icon = iconMap[toast.type]}
      <div class="pointer-events-auto flex items-center gap-2.5 px-4 py-3 rounded-xl border shadow-lg backdrop-blur-sm {colorMap[toast.type]}">
        <Icon class="w-4 h-4 shrink-0" />
        <span class="text-sm font-medium text-base-content flex-1">{toast.message}</span>
        <button
          onclick={() => removeToast(toast.id)}
          class="p-0.5 rounded hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none"
        >
          <X class="w-3.5 h-3.5 text-base-content/50" />
        </button>
      </div>
    {/each}
  </div>
{/if}
