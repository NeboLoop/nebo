<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { engineService, openLoginItems } from '$lib/api/engineService';

  let { collapsed = false } = $props();
  let show = $state(false);
  // The service can't run on this computer at all (Linux with no systemd
  // user session): plain words, no button.
  let unavailable = $state(false);

  // Nebo switched off in Login Items runs only while the app is open. The
  // app notices when it is switched on again, so this looks every so often.
  onMount(() => {
    const look = async () => {
      const s = await engineService();
      show = s?.needsApproval ?? false;
      unavailable = s?.unavailable ?? false;
    };
    look();
    const timer = setInterval(look, 15000);
    return () => clearInterval(timer);
  });
</script>

{#if unavailable && !collapsed}
  <div class="border-t border-base-300 shrink-0 py-2.5 px-3.5">
    <div class="text-xs text-base-content/70">{$t('loginItemsBanner.unavailable')}</div>
  </div>
{:else if show}
  <div class="border-t border-base-300 shrink-0 {collapsed ? 'py-2.5 flex justify-center' : 'py-2.5 px-3.5'}">
    {#if collapsed}
      <button class="btn btn-ghost btn-xs" onclick={openLoginItems} title={$t('loginItemsBanner.text')}>
        {$t('loginItemsBanner.open')}
      </button>
    {:else}
      <div class="text-xs text-base-content/70 mb-2">{$t('loginItemsBanner.text')}</div>
      <button class="btn btn-xs btn-outline w-full" onclick={openLoginItems}>{$t('loginItemsBanner.open')}</button>
    {/if}
  </div>
{/if}
