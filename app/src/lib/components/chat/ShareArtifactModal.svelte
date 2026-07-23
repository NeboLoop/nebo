<!--
  Share Artifact Modal — shares a Work-panel artifact into the loop:
  post to a loop channel or DM a loop member (bot/agent). Targets come
  from /neboai/share/targets; the send reuses the run-artifact upload
  pathway server-side.
-->

<script lang="ts">
  import { t } from 'svelte-i18n';
  import { neboAIShareTargets, neboAIShareArtifact } from '$lib/api/nebo';
  import type { ShareChannel, ShareMember } from '$lib/api/neboComponents';
  import { addToast } from '$lib/stores/toast';

  interface Props {
    show?: boolean;
    /** Artifact reference (/api/v1/files/... URL) to share. */
    url: string;
    title: string;
  }

  let { show = $bindable(false), url, title }: Props = $props();

  let loading = $state(false);
  let sending = $state(false);
  let connected = $state(true);
  let channels = $state<ShareChannel[]>([]);
  let members = $state<ShareMember[]>([]);
  /** Selected target: "c:<channelId>" or "m:<botId>". */
  let selected = $state('');
  let message = $state('');

  $effect(() => {
    if (show) loadTargets();
  });

  async function loadTargets() {
    loading = true;
    selected = '';
    message = '';
    try {
      const res = await neboAIShareTargets();
      connected = res.connected;
      channels = res.channels ?? [];
      members = res.members ?? [];
    } catch {
      connected = false;
      channels = [];
      members = [];
    } finally {
      loading = false;
    }
  }

  const selectedName = $derived.by(() => {
    if (selected.startsWith('c:')) return channels.find((c) => c.channelId === selected.slice(2))?.channelName ?? '';
    if (selected.startsWith('m:')) return members.find((m) => m.botId === selected.slice(2))?.botName ?? '';
    return '';
  });

  async function send() {
    if (!selected || sending) return;
    sending = true;
    try {
      await neboAIShareArtifact({
        artifact: url,
        text: message,
        channelId: selected.startsWith('c:') ? selected.slice(2) : '',
        toBotId: selected.startsWith('m:') ? selected.slice(2) : '',
      });
      addToast($t('chat.shareSuccess', { values: { name: selectedName } }), 'success');
      show = false;
    } catch (e) {
      addToast(e instanceof Error ? e.message : $t('chat.shareFailed'), 'error');
    } finally {
      sending = false;
    }
  }
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center" role="dialog" aria-modal="true">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm cursor-default border-none" onclick={() => (show = false)} aria-label={$t('common.close')}></button>
    <div class="relative rounded-2xl bg-base-100 w-full max-w-md shadow-xl mx-4">
      <div class="px-5 py-4 border-b border-base-300 flex items-center gap-2">
        <h3 class="text-base font-semibold flex-1 truncate">{$t('chat.shareTitle', { values: { title } })}</h3>
        <button
          class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70"
          onclick={() => (show = false)}
          aria-label={$t('common.close')}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>

      <div class="px-5 py-4 max-h-[50vh] overflow-y-auto">
        {#if loading}
          <div class="py-8 text-center"><span class="loading loading-spinner loading-md"></span></div>
        {:else if !connected}
          <div class="py-6 text-center text-xs text-base-content/70">{$t('chat.shareNotConnected')}</div>
        {:else if channels.length === 0 && members.length === 0}
          <div class="py-6 text-center text-xs text-base-content/70">{$t('chat.shareNoTargets')}</div>
        {:else}
          {#if channels.length > 0}
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('chat.shareChannels')}</div>
            <div class="flex flex-col gap-1 mb-4">
              {#each channels as c (c.channelId)}
                <button
                  class="flex items-center gap-2 w-full px-3 py-2 rounded-lg text-left cursor-pointer transition-colors {selected === `c:${c.channelId}` ? 'bg-primary/10 border border-primary/40' : 'bg-base-200/50 border border-transparent hover:bg-base-200'}"
                  onclick={() => (selected = `c:${c.channelId}`)}
                >
                  <span class="text-base-content/50 font-mono text-sm shrink-0">#</span>
                  <span class="text-sm font-medium truncate">{c.channelName}</span>
                  <span class="text-xs text-base-content/50 truncate ml-auto">{c.loopName}</span>
                </button>
              {/each}
            </div>
          {/if}
          {#if members.length > 0}
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('chat.shareMembers')}</div>
            <div class="flex flex-col gap-1">
              {#each members as m (`${m.loopId}:${m.botId}`)}
                <button
                  class="flex items-center gap-2 w-full px-3 py-2 rounded-lg text-left cursor-pointer transition-colors {selected === `m:${m.botId}` ? 'bg-primary/10 border border-primary/40' : 'bg-base-200/50 border border-transparent hover:bg-base-200'}"
                  onclick={() => (selected = `m:${m.botId}`)}
                >
                  <span class="w-2 h-2 rounded-full shrink-0 {m.isOnline ? 'bg-success' : 'bg-base-content/20'}"></span>
                  <span class="text-sm font-medium truncate">{m.botName}</span>
                  <span class="text-xs text-base-content/50 truncate ml-auto">{m.loopName}</span>
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      <div class="px-5 py-4 border-t border-base-300 flex flex-col gap-3">
        <input
          type="text"
          class="input input-sm input-bordered w-full text-sm"
          placeholder={$t('chat.shareMessagePlaceholder')}
          bind:value={message}
        />
        <div class="flex justify-end gap-2">
          <button class="btn btn-sm btn-ghost" onclick={() => (show = false)}>{$t('common.cancel')}</button>
          <button class="btn btn-sm btn-primary" disabled={!selected || sending} onclick={send}>
            {#if sending}<span class="loading loading-spinner loading-xs"></span>{/if}
            {$t('chat.shareSend')}
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}
