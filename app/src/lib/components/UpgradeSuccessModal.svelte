<!--
  Upgrade Success Modal
  Shown when a plan_changed WebSocket event fires after a successful payment
-->

<script lang="ts">
  import CheckCircle from 'lucide-svelte/icons/check-circle';

  interface Props {
    show?: boolean;
    plan: string;
    onclose?: () => void;
  }

  let {
    show = $bindable(false),
    plan,
    onclose
  }: Props = $props();

  function handleClose() {
    show = false;
    onclose?.();
  }

  const planDisplay = $derived(plan ? plan.charAt(0).toUpperCase() + plan.slice(1) : 'new');
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center" role="dialog" aria-modal="true">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm cursor-default border-none" onclick={handleClose} aria-label="Close"></button>
    <div class="relative rounded-2xl bg-base-100 w-full max-w-sm shadow-xl">
      <div class="px-6 py-8">
        <div class="flex flex-col items-center text-center gap-4">
          <div class="w-16 h-16 rounded-full bg-success/15 flex items-center justify-center">
            <CheckCircle class="w-8 h-8 text-success" />
          </div>

          <div>
            <h3 class="text-base font-semibold text-base-content">Plan Upgraded</h3>
            <p class="text-xs text-base-content/70 mt-1">You're now on the {planDisplay} plan.</p>
          </div>

          <button
            class="h-10 px-6 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all cursor-pointer border-none"
            onclick={handleClose}
          >
            Got it
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}
