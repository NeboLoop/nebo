<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import AlertCircle from 'lucide-svelte/icons/alert-circle';
  import Mail from 'lucide-svelte/icons/mail';
  import CheckCircle2 from 'lucide-svelte/icons/check-circle-2';
  import X from 'lucide-svelte/icons/x';
  import { goto } from '$lib/nav';
  import NotificationModal from '$lib/components/NotificationModal.svelte';
  import {
    notifications, loadNotifications, markAsRead, removeNotification, type Notification,
  } from '$lib/stores/notifications';

  let selected = $state<Notification | null>(null);

  onMount(() => { loadNotifications(); });

  // v1 tiers sourced from the notification stream. "Needs you" = failures/warnings
  // (things you must resolve); everything else is a delivery you can read.
  // ponytail: the "Done" tier (autonomous runs) comes from the runs feed — wire it
  // when the Inbox absorbs runs, not before; faking it now would lie about coverage.
  const needsYou = $derived($notifications.filter(n => n.type === 'error' || n.type === 'warning'));
  const delivered = $derived($notifications.filter(n => n.type !== 'error' && n.type !== 'warning'));

  const typeColors: Record<string, string> = {
    agent: 'bg-success', system: 'bg-info', warning: 'bg-warning', error: 'bg-error',
  };

  function open(n: Notification) {
    markAsRead(n.id);
    selected = n;
  }
  function takeAction(n: Notification) {
    selected = null;
    if (n.link) goto(n.link);
  }
</script>

<svelte:head><title>{$t('inbox.title')}</title></svelte:head>

<div class="flex-1 overflow-y-auto bg-base-100">
  <div class="max-w-2xl mx-auto px-4 md:px-6 py-6">
    <h1 class="text-base font-semibold mb-5">{$t('inbox.title')}</h1>

    {#if $notifications.length === 0}
      <div class="flex flex-col items-center justify-center text-center py-24 gap-2">
        <CheckCircle2 class="w-8 h-8 text-success/60" />
        <div class="text-sm font-medium">{$t('inbox.empty')}</div>
        <div class="text-xs text-base-content/50">{$t('inbox.emptyDesc')}</div>
      </div>
    {:else}
      <!-- Needs you -->
      <section class="mb-6">
        <div class="flex items-center gap-2 mb-2">
          <AlertCircle class="w-3.5 h-3.5 text-error" />
          <h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('inbox.needsYou')}</h2>
          {#if needsYou.length > 0}<span class="text-xs text-base-content/40 font-mono">{needsYou.length}</span>{/if}
        </div>
        {#if needsYou.length === 0}
          <div class="text-xs text-base-content/50 px-3 py-4">{$t('inbox.emptyDesc')}</div>
        {:else}
          <div class="flex flex-col gap-1">
            {#each needsYou as n (n.id)}
              {@render item(n)}
            {/each}
          </div>
        {/if}
      </section>

      <!-- Delivered -->
      {#if delivered.length > 0}
        <section class="mb-6">
          <div class="flex items-center gap-2 mb-2">
            <Mail class="w-3.5 h-3.5 text-info" />
            <h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('inbox.delivered')}</h2>
            <span class="text-xs text-base-content/40 font-mono">{delivered.length}</span>
          </div>
          <div class="flex flex-col gap-1">
            {#each delivered as n (n.id)}
              {@render item(n)}
            {/each}
          </div>
        </section>
      {/if}
    {/if}
  </div>
</div>

{#snippet item(n: Notification)}
  <div
    class="group flex items-start gap-3 px-3 py-3 rounded-lg border border-base-300 bg-base-100 hover:bg-base-200/50 transition-colors cursor-pointer {n.read ? '' : 'bg-primary/5'}"
    onclick={() => open(n)}
    onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(n); } }}
    role="button"
    tabindex="0"
  >
    <div class="w-2 h-2 rounded-full mt-1.5 shrink-0 {typeColors[n.type] || 'bg-info'}"></div>
    <div class="flex-1 min-w-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium truncate {n.read ? 'text-base-content/70' : 'text-base-content'}">{n.title}</span>
        <span class="text-xs text-base-content/50 font-mono shrink-0 ml-auto">{n.time}</span>
      </div>
      <p class="text-xs text-base-content/60 truncate mt-0.5">{n.message}</p>
    </div>
    <button
      onclick={(e) => { e.stopPropagation(); removeNotification(n.id); }}
      class="p-1 rounded hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none shrink-0 opacity-0 group-hover:opacity-100"
      aria-label={$t('notifications.closeNotification')}
    >
      <X class="w-3 h-3 text-base-content/40" />
    </button>
  </div>
{/snippet}

{#if selected}
  <NotificationModal notif={selected} onClose={() => (selected = null)} onAction={takeAction} />
{/if}
