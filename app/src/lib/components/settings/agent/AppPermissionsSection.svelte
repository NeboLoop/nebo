<!--
  AppPermissionsSection — what an app may do beyond its own page, in plain
  words (Settings → the app → Permissions). For an app from the marketplace
  each request is Allow or Off, as the owner answered it, and he can change
  any of them; an app his own employees built does what it asks for.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import Ban from 'lucide-svelte/icons/ban';
  import * as api from '$lib/api/nebo';
  import type { AppPermission, AppPermissions } from '$lib/api/nebo';
  import { permissionWords } from '$lib/apps/consent';

  let { agentId, name }: { agentId: string; name: string } = $props();

  let page = $state<AppPermissions | null>(null);
  let busy = $state(false);

  $effect(() => {
    const id = agentId;
    api.getAppPermissions(id).then((p) => (page = p)).catch(() => (page = null));
  });

  async function set(p: AppPermission, allowed: boolean) {
    if (busy) return;
    busy = true;
    try {
      page = await api.answerAppPermission(agentId, { permission: p.permission, allowed });
    } finally {
      busy = false;
    }
  }
</script>

{#if page && page.permissions.length}
  <section class="mb-4">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('appPermissions.sectionTitle')}</div>
    <div class="text-xs text-base-content/70 mt-1">{page.own ? $t('appPermissions.ownHint') : $t('appPermissions.sectionHint')}</div>
    <ul class="divide-y divide-base-content/10 mt-1">
      {#each page.permissions as p (p.permission)}
        {@const words = permissionWords(p, name, false)}
        <li class="flex items-center gap-3 py-2 flex-wrap">
          <span class="flex-1 min-w-0 text-sm">
            {$t(words.key, { values: words.values })}
            {#if p.state === 'asking'}<span class="badge badge-ghost badge-xs ms-1.5 align-middle">{$t('appPermissions.notAnswered')}</span>{/if}
          </span>
          {#if !page.own}
            <div class="join shrink-0" role="group" aria-label={$t(words.key, { values: words.values })}>
              <button type="button" class="btn btn-xs join-item {p.state === 'allowed' ? 'btn-active text-success' : 'btn-ghost text-base-content/50'}" aria-pressed={p.state === 'allowed'} disabled={busy} onclick={() => set(p, true)}>
                <Check class="w-3 h-3" /><span>{$t('appPermissions.allow')}</span>
              </button>
              <button type="button" class="btn btn-xs join-item {p.state === 'refused' ? 'btn-active text-error' : 'btn-ghost text-base-content/50'}" aria-pressed={p.state === 'refused'} disabled={busy} onclick={() => set(p, false)}>
                <Ban class="w-3 h-3" /><span>{$t('appPermissions.off')}</span>
              </button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}
