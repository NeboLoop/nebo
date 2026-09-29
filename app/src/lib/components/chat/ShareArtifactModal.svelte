<!--
  Share Artifact Modal — shares a Work-panel file by link
  (https://neboai.com/s/<token>): who can open it (anyone with the link,
  a password, or only you), an optional expiry, copy, and turn off. The bot
  uploads the file through the one upload path and the hub keeps the link;
  see $lib/chat/shareLink.
-->

<script lang="ts">
  import { t, locale } from 'svelte-i18n';
  import type { FileShare } from '$lib/api/neboComponents';
  import { expiresAtFor, loadShareLink, saveShareLink, turnOffShareLink, type ShareAccess, type ShareExpiry } from '$lib/chat/shareLink';
  import { addToast } from '$lib/stores/toast';

  interface Props {
    show?: boolean;
    /** Artifact reference (/api/v1/files/... URL) to share. */
    url: string;
    title: string;
  }

  let { show = $bindable(false), url, title }: Props = $props();

  let loading = $state(false);
  let saving = $state(false);
  let share = $state<FileShare | null>(null);
  let access = $state<ShareAccess>('link');
  let password = $state('');
  let expiry = $state<ShareExpiry>('never');

  $effect(() => {
    if (show) load();
  });

  function adopt(s: FileShare | null) {
    share = s;
    access = s?.access ?? 'link';
    password = '';
    expiry = s?.expiresAt ? 'keep' : 'never';
  }

  async function load() {
    loading = true;
    try {
      adopt(await loadShareLink(url));
    } catch (e) {
      adopt(null);
      addToast(e instanceof Error ? e.message : $t('chat.shareFailed'), 'error');
    } finally {
      loading = false;
    }
  }

  const accessOptions: { value: ShareAccess; label: string; hint: string }[] = $derived([
    { value: 'link', label: $t('chat.shareAccessLink'), hint: $t('chat.shareAccessLinkHint') },
    { value: 'password', label: $t('chat.shareAccessPassword'), hint: $t('chat.shareAccessPasswordHint') },
    { value: 'private', label: $t('chat.shareAccessPrivate'), hint: $t('chat.shareAccessPrivateHint') },
  ]);

  const keptDate = $derived(
    share?.expiresAt ? new Date(share.expiresAt).toLocaleDateString($locale ?? undefined, { dateStyle: 'medium' }) : ''
  );

  // A password link needs one to exist: typed now, or already on the link.
  const needsPassword = $derived(access === 'password' && !password && !share?.hasPassword);
  const changed = $derived(
    !share || access !== share.access || password !== '' || expiresAtFor(expiry, share.expiresAt) !== share.expiresAt
  );

  async function save() {
    if (saving || needsPassword || !changed) return;
    saving = true;
    try {
      const created = !share;
      adopt(await saveShareLink(url, access, password, expiresAtFor(expiry, share?.expiresAt ?? '')));
      if (created) await copy();
    } catch (e) {
      addToast(e instanceof Error ? e.message : $t('chat.shareFailed'), 'error');
    } finally {
      saving = false;
    }
  }

  async function copy() {
    if (!share) return;
    try {
      await navigator.clipboard.writeText(share.url);
      addToast($t('chat.shareCopied'), 'success');
    } catch {
      addToast($t('chat.copyFailed'), 'error');
    }
  }

  async function turnOff() {
    if (saving) return;
    saving = true;
    try {
      await turnOffShareLink(url);
      adopt(null);
      addToast($t('chat.shareTurnedOff'), 'success');
    } catch (e) {
      addToast(e instanceof Error ? e.message : $t('chat.shareFailed'), 'error');
    } finally {
      saving = false;
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

      {#if loading}
        <div class="py-10 text-center"><span class="loading loading-spinner loading-md"></span></div>
      {:else}
        <div class="px-5 py-4 flex flex-col gap-4">
          {#if share}
            <div class="flex items-center gap-2">
              <input type="text" class="input input-sm input-bordered flex-1 min-w-0 text-sm" readonly value={share.url} aria-label={share.url} onfocus={(e) => e.currentTarget.select()} />
              <button class="btn btn-sm btn-primary" onclick={copy}>{$t('chat.shareCopy')}</button>
            </div>
          {/if}

          <fieldset class="flex flex-col gap-1">
            <legend class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('chat.shareWho')}</legend>
            {#each accessOptions as opt (opt.value)}
              <label
                class="flex items-start gap-3 w-full px-3 py-2 rounded-lg cursor-pointer transition-colors {access === opt.value ? 'bg-primary/10 border border-primary/40' : 'bg-base-200/50 border border-transparent hover:bg-base-200'}"
              >
                <input type="radio" class="radio radio-sm radio-primary mt-0.5" name="share-access" value={opt.value} bind:group={access} />
                <span class="flex flex-col">
                  <span class="text-sm font-medium">{opt.label}</span>
                  <span class="text-xs text-base-content/60">{opt.hint}</span>
                </span>
              </label>
            {/each}
          </fieldset>

          {#if access === 'password'}
            <input
              type="password"
              class="input input-sm input-bordered w-full text-sm"
              autocomplete="new-password"
              placeholder={share?.hasPassword ? $t('chat.sharePasswordKeep') : $t('chat.sharePasswordPlaceholder')}
              aria-label={$t('chat.shareAccessPassword')}
              bind:value={password}
            />
          {/if}

          <label class="flex items-center gap-3">
            <span class="text-sm flex-1">{$t('chat.shareExpires')}</span>
            <select class="select select-sm select-bordered text-sm" bind:value={expiry}>
              <option value="never">{$t('chat.shareExpiresNever')}</option>
              {#if share?.expiresAt}
                <option value="keep">{$t('chat.shareExpiresKeep', { values: { date: keptDate } })}</option>
              {/if}
              <option value="1">{$t('chat.shareExpires1')}</option>
              <option value="7">{$t('chat.shareExpires7')}</option>
              <option value="30">{$t('chat.shareExpires30')}</option>
            </select>
          </label>
        </div>

        <div class="px-5 py-4 border-t border-base-300 flex items-center gap-2">
          {#if share}
            <button class="btn btn-sm btn-ghost text-error" disabled={saving} onclick={turnOff}>{$t('chat.shareTurnOff')}</button>
          {/if}
          <div class="flex-1"></div>
          <button class="btn btn-sm btn-primary" disabled={saving || needsPassword || !changed} onclick={save}>
            {#if saving}<span class="loading loading-spinner loading-xs"></span>{/if}
            {share ? $t('chat.shareSave') : $t('chat.shareCreate')}
          </button>
        </div>
      {/if}
    </div>
  </div>
{/if}
