<script lang="ts">
  import { t } from 'svelte-i18n';
  import CheckCircle from 'lucide-svelte/icons/check-circle';
  import ExternalLink from 'lucide-svelte/icons/external-link';

  interface Props {
    show: boolean;
    pluginName?: string;
    onclose?: () => void;
  }

  let {
    show = $bindable(false),
    pluginName = 'Plugin',
    onclose,
  }: Props = $props();

  let connecting = $state(false);
  let connected = $state(false);

  function handleConnect() {
    connecting = true;
    setTimeout(() => {
      connecting = false;
      connected = true;
    }, 1500);
  }

  function handleClose() {
    show = false;
    // Reset state for next open
    setTimeout(() => {
      connecting = false;
      connected = false;
    }, 300);
    onclose?.();
  }

  function handleSkip() {
    handleClose();
  }
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation" onclick={handleClose} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); handleClose(); } }}></div>

    <div class="relative w-full max-w-sm rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden">
      <div class="px-6 py-6">
        {#if connected}
          <!-- Success state -->
          <div class="text-center">
            <div class="w-14 h-14 rounded-full bg-success/15 flex items-center justify-center mx-auto mb-4">
              <CheckCircle class="w-7 h-7 text-success" />
            </div>
            <h3 class="text-lg font-bold mb-1">{$t('common.connected')}</h3>
            <p class="text-xs text-base-content/50 mb-6">{$t('oauthConnect.connectedAs', { values: { email: 'alex@acme.co' } })}</p>
            <button
              onclick={handleClose}
              class="w-full py-2.5 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
            >
              {$t('common.done')}
            </button>
          </div>
        {:else}
          <!-- Connect prompt -->
          <div class="text-center">
            <div class="w-14 h-14 rounded-xl bg-base-200 flex items-center justify-center mx-auto mb-4 text-2xl">
              <ExternalLink class="w-6 h-6 text-base-content/50" />
            </div>
            <h3 class="text-lg font-bold mb-1">{$t('oauthConnect.requiresAuthorization', { values: { name: pluginName } })}</h3>
            <p class="text-xs text-base-content/50 mb-6">{$t('oauthConnect.description', { values: { name: pluginName } })}</p>

            <button
              onclick={handleConnect}
              disabled={connecting}
              class="w-full py-2.5 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-50 mb-3"
            >
              {#if connecting}
                <span class="loading loading-spinner loading-sm"></span> {$t('settingsPlugins.connecting')}
              {:else}
                {$t('oauthConnect.connectName', { values: { name: pluginName } })}
              {/if}
            </button>
            <button
              onclick={handleSkip}
              class="text-xs text-base-content/50 hover:text-base-content transition-colors cursor-pointer bg-transparent border-none"
            >
              {$t('oauthConnect.connectLater')}
            </button>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}
