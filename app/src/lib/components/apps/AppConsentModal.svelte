<!--
  AppConsentModal — asks the owner, in plain words, about what an app from
  the marketplace wants to do beyond its own page (`$lib/apps/consent`).
  Each request is answered on its own; the app opens when he is done or
  puts it off ("Not now": asked again next time).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import * as api from '$lib/api/nebo';
  import { consentAsk, permissionWords } from '$lib/apps/consent';
  import type { AppPermission } from '$lib/api/nebo';

  let answered = $state<Record<string, boolean>>({});
  let busy = $state(false);

  const ask = $derived($consentAsk);
  const left = $derived(ask ? ask.permissions.filter((p) => !(p.permission in answered)) : []);

  function close() {
    const open = ask;
    consentAsk.set(null);
    answered = {};
    open?.done();
  }

  async function answer(p: AppPermission, allowed: boolean) {
    if (!ask || busy) return;
    busy = true;
    try {
      await api.answerAppPermission(ask.agentId, { permission: p.permission, allowed });
      answered = { ...answered, [p.permission]: allowed };
    } finally {
      busy = false;
    }
    if (ask.permissions.every((x) => x.permission in answered)) close();
  }
</script>

{#if ask}
  <div class="modal modal-open" role="dialog" aria-modal="true" aria-labelledby="app-consent-title">
    <div class="modal-box max-w-md">
      <h3 id="app-consent-title" class="text-base font-semibold">{$t('appPermissions.title', { values: { name: ask.name } })}</h3>
      <p class="text-sm text-base-content/70 mt-2">{$t('appPermissions.intro')}</p>
      <ul class="divide-y divide-base-content/10 mt-3">
        {#each left as p (p.permission)}
          {@const words = permissionWords(p, ask.name, true)}
          <li class="flex items-center gap-3 py-2.5 flex-wrap">
            <span class="flex-1 min-w-0 text-sm">{$t(words.key, { values: words.values })}</span>
            <div class="flex gap-2 shrink-0">
              <button type="button" class="btn btn-sm btn-ghost" disabled={busy} onclick={() => answer(p, false)}>{$t('appPermissions.deny')}</button>
              <button type="button" class="btn btn-sm btn-primary" disabled={busy} onclick={() => answer(p, true)}>{$t('appPermissions.allow')}</button>
            </div>
          </li>
        {/each}
      </ul>
      <div class="modal-action">
        <button type="button" class="btn btn-sm btn-ghost" onclick={close}>{$t('appPermissions.notNow')}</button>
      </div>
    </div>
    <button type="button" class="modal-backdrop" aria-label={$t('appPermissions.notNow')} onclick={close}></button>
  </div>
{/if}
