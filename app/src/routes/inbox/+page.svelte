<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { marked } from 'marked';
  import CheckCircle2 from 'lucide-svelte/icons/check-circle-2';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Mail from 'lucide-svelte/icons/mail';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import { goto } from '$lib/nav';
  import {
    notifications, unreadCount, loadNotifications, markAsRead, markAllRead, removeNotification, type Notification,
  } from '$lib/stores/notifications';

  let selectedId = $state<string | null>(null);
  let copied = $state(false);

  onMount(() => { loadNotifications(); });

  const typeColors: Record<string, string> = {
    agent: 'bg-success', system: 'bg-info', warning: 'bg-warning', error: 'bg-error',
  };

  const sorted = $derived([...$notifications].sort((a, b) => b.createdAt - a.createdAt));
  const selected = $derived(sorted.find(n => n.id === selectedId) ?? null);

  function open(n: Notification) {
    markAsRead(n.id);
    selectedId = n.id;
    copied = false;
  }
  function remove(id: string) {
    if (selectedId === id) selectedId = null;
    removeNotification(id);
  }
  async function copyBody(n: Notification) {
    await navigator.clipboard.writeText(n.message);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<svelte:head><title>{$t('inbox.title')}</title></svelte:head>

<div class="flex-1 flex min-h-0 bg-base-100">
  <!-- Message list (email-style rows). On mobile the list and reading pane swap full-screen. -->
  <div class="w-full md:w-80 lg:w-96 shrink-0 border-r border-base-300 bg-base-200/50 flex-col min-h-0 {selected ? 'hidden md:flex' : 'flex'}">
    <div class="flex items-center justify-between h-12 px-4 border-b border-base-content/10 shrink-0">
      <h1 class="text-base font-semibold">{$t('inbox.title')}</h1>
      {#if $unreadCount > 0}
        <button class="btn btn-ghost btn-xs text-base-content/70" onclick={markAllRead}>
          {$t('notifications.markAllRead')}
        </button>
      {/if}
    </div>
    <div class="flex-1 overflow-y-auto">
      {#if sorted.length === 0}
        <div class="flex flex-col items-center justify-center text-center py-24 gap-2 px-4">
          <CheckCircle2 class="w-8 h-8 text-success/60" />
          <div class="text-sm font-medium">{$t('inbox.empty')}</div>
          <div class="text-xs text-base-content/50">{$t('inbox.emptyDesc')}</div>
        </div>
      {:else}
        {#each sorted as n (n.id)}
          <div
            class="group relative flex items-start gap-2.5 px-4 py-3 border-b border-base-content/10 cursor-pointer transition-colors {selectedId === n.id
              ? 'bg-base-100 border-l-2 border-l-primary pl-[14px]'
              : 'border-l-2 border-l-transparent hover:bg-base-200 pl-[14px]'}"
            onclick={() => open(n)}
            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(n); } }}
            role="button"
            tabindex="0"
          >
            <div class="w-2 h-2 rounded-full mt-1.5 shrink-0 {n.read ? 'bg-transparent' : typeColors[n.type] || 'bg-info'}"></div>
            <div class="flex-1 min-w-0">
              <div class="flex items-baseline gap-2">
                <span class="text-sm truncate {n.read ? 'font-normal text-base-content/70' : 'font-semibold text-base-content'}">{n.title}</span>
                <span class="text-xs text-base-content/50 font-mono shrink-0 ml-auto">{n.time}</span>
              </div>
              <p class="text-xs text-base-content/60 truncate mt-0.5">{n.message}</p>
            </div>
            <button
              onclick={(e) => { e.stopPropagation(); remove(n.id); }}
              class="absolute right-2 bottom-2 p-1 rounded hover:bg-base-content/10 transition-opacity cursor-pointer bg-base-200 border-none opacity-0 group-hover:opacity-100"
              aria-label={$t('notifications.closeNotification')}
            >
              <Trash2 class="w-3 h-3 text-base-content/40" />
            </button>
          </div>
        {/each}
      {/if}
    </div>
  </div>

  <!-- Reading pane -->
  <div class="flex-1 min-w-0 flex-col min-h-0 {selected ? 'flex' : 'hidden md:flex'}">
    {#if selected}
      <div class="flex items-center gap-2 h-12 px-4 border-b border-base-content/10 shrink-0">
        <button class="md:hidden p-1.5 rounded hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={() => (selectedId = null)} aria-label={$t('common.close')}>
          <ArrowLeft class="w-4 h-4 text-base-content/70" />
        </button>
        <div class="w-2 h-2 rounded-full shrink-0 {typeColors[selected.type] || 'bg-info'}"></div>
        <span class="text-sm font-medium truncate">{selected.title}</span>
        <span class="text-xs text-base-content/50 font-mono shrink-0">{selected.time}</span>
        <div class="ml-auto flex items-center gap-1 shrink-0">
          {#if selected.link}
            <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected?.link && goto(selected.link)}>
              <ExternalLink class="w-3.5 h-3.5" />
              {$t('common.open')}
            </button>
          {/if}
          <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected && copyBody(selected)}>
            {#if copied}<Check class="w-3.5 h-3.5 text-success" />{:else}<Copy class="w-3.5 h-3.5" />{/if}
            {$t('common.copy')}
          </button>
          <button class="btn btn-ghost btn-xs" onclick={() => selected && remove(selected.id)} aria-label={$t('common.delete')}>
            <Trash2 class="w-3.5 h-3.5 text-base-content/50" />
          </button>
        </div>
      </div>
      <div class="flex-1 overflow-y-auto">
        <div class="max-w-2xl mx-auto px-6 py-6">
          <div class="prose prose-sm max-w-none [&>:first-child]:mt-0">
            {@html marked.parse(selected.message, { async: false })}
          </div>
        </div>
      </div>
    {:else}
      <div class="flex-1 flex flex-col items-center justify-center text-center gap-2 text-base-content/40">
        <Mail class="w-8 h-8" />
        <div class="text-xs">{$t('inbox.emptyDesc')}</div>
      </div>
    {/if}
  </div>
</div>
