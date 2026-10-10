<!--
  Share Artifact Modal — shares a Work-panel file by link
  (https://neboai.com/s/<token>): who can open it (anyone with the link,
  a password, or only you), an optional expiry, whether the link follows the
  file or keeps this version, publishing a web page as a site at
  <address>.nebo.page, copy, and turn off. The bot
  uploads the file through the one upload path and the hub keeps the link;
  see $lib/chat/shareLink.
-->

<script lang="ts">
  import { t, locale } from 'svelte-i18n';
  import type { FileShare } from '$lib/api/neboComponents';
  import { expiresAtFor, loadShareLink, saveShareLink, siteAddressFor, turnOffShareLink, type ShareAccess, type ShareExpiry, type ShareState } from '$lib/chat/shareLink';
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
  let live = $state(true);
  let outdated = $state(false);
  let publish = $state(false);
  let address = $state('');

  // A web page can also be published as a site of its own, at
  // <address>.nebo.page (the hub's sites domain).
  const SITES_DOMAIN = '.nebo.page';
  const isPage = $derived(/\.html?$/i.test(title) || /\.html?$/i.test(url));

  $effect(() => {
    if (show) load();
  });

  function adopt(st: ShareState) {
    const s = st.share;
    share = s;
    outdated = st.outdated;
    access = s?.access ?? 'link';
    password = '';
    expiry = s?.expiresAt ? 'keep' : 'never';
    live = s?.live ?? true;
    publish = !!s?.address;
    address = s?.address || siteAddressFor(title);
  }

  async function load() {
    loading = true;
    try {
      adopt(await loadShareLink(url));
    } catch (e) {
      adopt({ share: null, outdated: false });
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

  const updateOptions: { value: boolean; label: string; hint: string }[] = $derived([
    { value: true, label: $t('chat.shareLive'), hint: $t('chat.shareLiveHint') },
    { value: false, label: $t('chat.shareSnapshot'), hint: $t('chat.shareSnapshotHint') },
  ]);

  const keptDate = $derived(
    share?.expiresAt ? new Date(share.expiresAt).toLocaleDateString($locale ?? undefined, { dateStyle: 'medium' }) : ''
  );

  // A password link needs one to exist: typed now, or already on the link.
  const needsPassword = $derived(access === 'password' && !password && !share?.hasPassword);
  const changed = $derived(
    !share ||
      access !== share.access ||
      password !== '' ||
      live !== share.live ||
      publish !== !!share.address ||
      (publish && address !== share.address) ||
      expiresAtFor(expiry, share.expiresAt) !== share.expiresAt
  );

  async function save(newVersion = false) {
    if (saving || needsPassword || (!changed && !newVersion)) return;
    saving = true;
    try {
      const created = !share;
      const site = publish ? address.trim().toLowerCase() : share?.address ? '' : undefined;
      adopt(await saveShareLink(url, access, password, expiresAtFor(expiry, share?.expiresAt ?? ''), live, newVersion, site));
      if (created) await copy();
    } catch (e) {
      addToast(e instanceof Error ? e.message : $t('chat.shareFailed'), 'error');
    } finally {
      saving = false;
    }
  }

  async function copy(text = share?.url) {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
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
      adopt({ share: null, outdated: false });
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
              <button class="btn btn-sm btn-primary" onclick={() => copy()}>{$t('chat.shareCopy')}</button>
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

          <fieldset class="flex flex-col gap-1">
            <legend class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('chat.shareUpdates')}</legend>
            {#each updateOptions as opt (opt.value)}
              <label
                class="flex items-start gap-3 w-full px-3 py-2 rounded-lg cursor-pointer transition-colors {live === opt.value ? 'bg-primary/10 border border-primary/40' : 'bg-base-200/50 border border-transparent hover:bg-base-200'}"
              >
                <input type="radio" class="radio radio-sm radio-primary mt-0.5" name="share-updates" value={opt.value} bind:group={live} />
                <span class="flex flex-col">
                  <span class="text-sm font-medium">{opt.label}</span>
                  <span class="text-xs text-base-content/60">{opt.hint}</span>
                </span>
              </label>
            {/each}
            {#if share && !share.live && outdated}
              <div class="flex items-center gap-2 px-3 pt-1">
                <span class="text-xs text-base-content/70 flex-1">{$t('chat.shareOutdated')}</span>
                <button class="btn btn-xs btn-outline" disabled={saving || needsPassword} onclick={() => save(true)}>{$t('chat.shareNewVersion')}</button>
              </div>
            {/if}
          </fieldset>

          {#if isPage}
            <fieldset class="flex flex-col gap-2">
              <label class="flex items-start gap-3 cursor-pointer">
                <input type="checkbox" class="checkbox checkbox-sm checkbox-primary mt-0.5" bind:checked={publish} />
                <span class="flex flex-col">
                  <span class="text-sm font-medium">{$t('chat.sharePublish')}</span>
                  <span class="text-xs text-base-content/60">{$t('chat.sharePublishHint')}</span>
                </span>
              </label>
              {#if publish}
                <label class="input input-sm input-bordered flex items-center gap-1 text-sm">
                  <input type="text" class="grow min-w-0" spellcheck="false" autocomplete="off" aria-label={$t('chat.sharePublishAddress')} bind:value={address} />
                  <span class="text-base-content/50">{SITES_DOMAIN}</span>
                </label>
                {#if share?.siteUrl && share.address === address}
                  <div class="flex items-center gap-2">
                    <a class="link text-sm flex-1 truncate" href={share.siteUrl} target="_blank" rel="noopener">{share.siteUrl}</a>
                    <button class="btn btn-xs btn-outline" onclick={() => copy(share?.siteUrl)}>{$t('chat.shareCopy')}</button>
                  </div>
                {/if}
              {/if}
            </fieldset>
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
          <button class="btn btn-sm btn-primary" disabled={saving || needsPassword || !changed} onclick={() => save()}>
            {#if saving}<span class="loading loading-spinner loading-xs"></span>{/if}
            {share ? $t('chat.shareSave') : $t('chat.shareCreate')}
          </button>
        </div>
      {/if}
    </div>
  </div>
{/if}
