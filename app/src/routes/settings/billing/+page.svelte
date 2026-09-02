<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import * as api from '$lib/api/nebo';
  import type { AccountStatusResponse } from '$lib/api/neboComponents';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import { openWebBilling } from '$lib/billing';

  let isLoading = $state(true);
  let status = $state<AccountStatusResponse | null>(null);

  onMount(() => {
    (async () => {
      try {
        status = (await api.neboAIAccountStatus()) as AccountStatusResponse;
      } catch {
        status = null;
      } finally {
        isLoading = false;
      }
    })();
  });

  onWsEvent<{ plan?: string }>('plan_changed', (d) => {
    if (d?.plan && status) status = { ...status, plan: d.plan };
  });

  const currentPlan = $derived((status?.plan || 'free').toLowerCase());
  const planName = $derived(currentPlan.charAt(0).toUpperCase() + currentPlan.slice(1));
</script>

<SettingsHeader title={$t('settingsBilling.title')} description={$t('settingsBilling.webOnly')} />

{#if isLoading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/70">{$t('settingsBilling.loadingBilling')}</span>
  </div>
{:else if !status?.connected}
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
    <p class="text-xs text-base-content/70">{$t('settingsBilling.connectForBilling')}</p>
    <a href="/settings/account" class="inline-block mt-3 text-sm font-medium text-primary hover:brightness-110 transition-all">
      {$t('settingsBilling.goToAccount')}
    </a>
  </div>
{:else}
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
    <div class="flex items-center justify-between gap-4 p-5">
      <div>
        <p class="text-sm font-semibold text-base-content">{$t('settingsBilling.planTitle', { values: { plan: planName } })}</p>
        <p class="text-xs text-base-content/50 mt-0.5">{$t('settingsBilling.webOnly')}</p>
      </div>
      <button
        onclick={openWebBilling}
        class="shrink-0 flex items-center gap-1.5 text-sm text-primary font-medium hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
      >
        {$t('settingsBilling.manageOnWeb')} <ExternalLink class="w-3.5 h-3.5" />
      </button>
    </div>
    <div class="flex items-center justify-between p-5">
      <p class="text-sm text-base-content">{$t('settingsUsage.title')}</p>
      <a href="/settings/usage" class="text-sm text-primary font-medium hover:brightness-110 transition-all">{$t('common.view')}</a>
    </div>
  </div>
{/if}
