<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { neboAIOauthStart, neboAIOauthStatus } from '$lib/api/index';
  import { get } from 'svelte/store';
  import Building2 from 'lucide-svelte/icons/building-2';
  import { botName, botRenameUrl, loadBotName, openBotRename, offerMatchingRename } from '$lib/stores/botName';
  import MapPin from 'lucide-svelte/icons/map-pin';
  import type { BotLocation } from '$lib/api/neboComponents';

  let user = $state({ name: '', email: '', displayName: '' });
  let connected = $state(true);
  let reconnecting = $state(false);
  let reconnectError = $state('');
  // Set when the server couldn't open a browser (headless/Android) AND the
  // client-side popup was blocked — renders a tappable sign-in link instead.
  let authUrl = $state('');
  let oauthPollInterval: ReturnType<typeof setInterval> | null = null;
  let oauthTimeout: ReturnType<typeof setTimeout> | null = null;

  // The bot's immutable, globally-unique id (full UUID) — shown read-only as the
  // bot's permanent identity. Never changes; independent of name and handle.
  let botId = $state('');

  // The bot's default id-based handle (`bot_<id8>`), independent of the display
  // name. Shown read-only as the bot's permanent identity.
  let defaultHandle = $state('');

  // The bot's own hosted email address, when its NeboAI account gives it one.
  let botEmail = $state('');

  // The bot's name (Bot settings → Name). It is the owner's: the owner
  // renames it on the NeboAI web app with their own session, and the web and
  // the phone show the same name. The bot and its primary employee start with
  // the same name; right after the bot is renamed, offer to rename the
  // primary too — never silently.
  let nameLoaded = $state(false);
  let primaryName = '';
  let primaryOffer = $state<string | null>(null);
  let primaryBusy = $state(false);
  let primaryError = $state('');
  let primaryRenamedTo = $state('');

  // Back from the web app: read the name again, and if the bot was renamed
  // while it matched the primary, offer the primary the same name.
  async function refreshBotName() {
    if (!nameLoaded) return;
    const before = get(botName);
    await loadBotName(true);
    const after = get(botName);
    if (offerMatchingRename(before, primaryName, after)) {
      primaryRenamedTo = '';
      primaryOffer = after;
    }
  }

  async function renamePrimary() {
    if (!primaryOffer || primaryBusy) return;
    primaryBusy = true;
    primaryError = '';
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgent('assistant', { name: primaryOffer });
      primaryName = primaryOffer;
      primaryRenamedTo = primaryOffer;
      primaryOffer = null;
    } catch (e) {
      primaryError = e instanceof Error ? e.message : $t('agentSettings.saveFailed');
    } finally {
      primaryBusy = false;
    }
  }

  // The bot's Location (Bot settings → Location): the office. Stored in
  // Nebo itself, so it needs no NeboAI account. This app has no geocoder, so
  // an address typed here is saved without coordinates, and the phone fills
  // them in the next time Nebo opens there.
  let location = $state<BotLocation | null>(null);
  let locationDraft = $state('');
  let locationBusy = $state(false);
  let locationError = $state('');
  let locationSaved = $state(false);

  async function saveLocation(label: string) {
    if (locationBusy) return;
    // The same address again keeps the coordinates it already has.
    if (label.trim() === (location?.label ?? '')) return;
    locationBusy = true;
    locationError = '';
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.updateLocation({ label: label.trim() });
      location = resp.location ?? null;
      locationDraft = location?.label ?? '';
      locationSaved = true;
      setTimeout(() => (locationSaved = false), 2000);
    } catch {
      locationError = $t('settingsAccount.botLocationSaveFailed');
    } finally {
      locationBusy = false;
    }
  }

  onDestroy(() => {
    if (oauthPollInterval) clearInterval(oauthPollInterval);
    if (oauthTimeout) clearTimeout(oauthTimeout);
  });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const status = await api.neboAIAccountStatus() as unknown as Record<string, unknown> | null;
      if (status) {
        connected = !!status.connected;
        if (status.email) user.email = String(status.email);
        if (status.displayName) {
          user.displayName = String(status.displayName);
          user.name = String(status.displayName);
        }
      }
    } catch { /* keep mock data */ }

    // Load the bot's permanent identity (default `bot_<id8>` handle + full id).
    try {
      const api = await import('$lib/api/nebo');
      const botStatus = (await api.neboAIBotStatus()) as { defaultHandle?: string; botId?: string };
      if (botStatus?.defaultHandle) defaultHandle = botStatus.defaultHandle;
      if (botStatus?.botId) botId = botStatus.botId;
    } catch { /* not connected — identity shows placeholder */ }

    try {
      const api = await import('$lib/api/nebo');
      const email = await api.neboAIBotEmail('');
      if (email?.address) botEmail = email.address;
    } catch { /* no hosted address — the row stays hidden */ }

    try {
      const api = await import('$lib/api/nebo');
      location = (await api.getLocation()).location ?? null;
      locationDraft = location?.label ?? '';
    } catch { /* the row shows empty; saving says so if the bot can't be reached */ }

    await loadBotName();
    nameLoaded = true;
    try {
      const api = await import('$lib/api/nebo');
      const primary = await api.getAgent('assistant');
      primaryName = (primary as { agent?: { name?: string } })?.agent?.name ?? '';
    } catch { /* no primary row: nothing to offer */ }
  });

  async function reconnect() {
    reconnecting = true;
    reconnectError = '';
    authUrl = '';
    try {
      const result = await neboAIOauthStart();
      const pendingState = result.state;

      // Headless server (Android/remote) can't open a browser — open it here.
      if (!result.opened) {
        const popup = window.open(result.authorizeUrl, '_blank', 'noopener');
        if (!popup) authUrl = result.authorizeUrl;
      }

      oauthTimeout = setTimeout(() => {
        if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
        reconnecting = false;
        reconnectError = $t('settingsAccount.connectionTimeout');
      }, 180_000);

      oauthPollInterval = setInterval(async () => {
        try {
          const status = await neboAIOauthStatus(pendingState);
          if (status?.status === 'complete') {
            if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
            if (oauthTimeout) { clearTimeout(oauthTimeout); oauthTimeout = null; }
            connected = true;
            reconnecting = false;
            if (status.email) user.email = status.email;
            if (status.displayName) { user.displayName = status.displayName; user.name = status.displayName; }
          } else if (status?.status === 'error') {
            if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
            if (oauthTimeout) { clearTimeout(oauthTimeout); oauthTimeout = null; }
            reconnecting = false;
            reconnectError = status.error || $t('settingsAccount.oauthFailed');
          } else if (status?.status === 'expired') {
            if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
            if (oauthTimeout) { clearTimeout(oauthTimeout); oauthTimeout = null; }
            reconnecting = false;
            reconnectError = $t('settingsAccount.oauthExpired');
          }
        } catch {
          // Poll error — keep trying
        }
      }, 2000);
    } catch (err) {
      reconnecting = false;
      reconnectError = err instanceof Error ? err.message : $t('settingsAccount.oauthStartFailed');
    }
  }

  async function disconnect() {
    try {
      const api = await import('$lib/api/nebo');
      await api.neboAIAccountDisconnect();
      connected = false;
    } catch { /* ignore */ }
  }

  async function handleDeleteAccount() {
    if (!confirm($t('settingsAccount.deleteConfirm'))) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.userDeleteAccount();
    } catch { /* ignore */ }
  }
</script>

<svelte:window onfocus={refreshBotName} />

<SettingsHeader title={$t('settingsAccount.neboaiAccount')} description={$t('settingsAccount.pageDescription')} />

<!-- Connection status + inline connect/disconnect action -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-2">
  <div class="flex items-center gap-3">
    <div class="w-10 h-10 rounded-lg bg-primary/20 text-primary grid place-items-center font-mono text-sm font-semibold">{user.name.charAt(0)}</div>
    <div class="flex-1 min-w-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium truncate">{user.displayName}</span>
        <span class="px-2 py-0.5 rounded text-xs font-semibold {connected ? 'bg-success/10 text-success' : 'bg-base-200 text-base-content/70'}">
          {connected ? $t('common.connected') : $t('common.disconnected')}
        </span>
      </div>
      <div class="text-xs text-base-content/70 truncate">{user.email}</div>
    </div>
    {#if connected}
      <button class="shrink-0 px-3 py-1.5 rounded-lg border border-error/20 text-sm font-medium text-error hover:bg-error/5 transition-colors cursor-pointer" onclick={disconnect}>{$t('settingsAccount.disconnect')}</button>
    {:else}
      <button
        class="shrink-0 px-3 py-1.5 rounded-lg border border-primary/30 text-sm font-medium text-primary hover:bg-primary/5 transition-colors cursor-pointer disabled:opacity-50"
        onclick={reconnect}
        disabled={reconnecting}
      >{reconnecting ? $t('settingsPlugins.connecting') : $t('oauth.connect')}</button>
    {/if}
  </div>
  {#if reconnectError}
    <div class="text-xs text-error mt-2">{reconnectError}</div>
  {/if}
  {#if authUrl && reconnecting}
    <a href={authUrl} target="_blank" rel="noopener" class="text-sm font-medium text-primary hover:underline mt-2 inline-block">{$t('oauth.continueInBrowser')} →</a>
  {/if}
</div>

<div class="mb-8">
  <a href="/settings/usage" class="text-sm font-medium text-primary hover:underline">{$t('settingsAccount.viewUsageArrow')}</a>
</div>

<!-- The bot's name -->
<div class="mb-8">
  <h3 class="text-base font-semibold mb-1 flex items-center gap-2"><Building2 class="w-4 h-4 text-base-content/70" aria-hidden="true" />{$t('settingsAccount.botName')}</h3>
  <p class="text-xs text-base-content/70 mb-2.5">{$t('settingsAccount.botNameDesc')}</p>
  {#if nameLoaded && !connected}
    <div class="text-sm text-base-content/60">{$t('settingsAccount.botNameNotConnected')}</div>
  {:else}
    <div class="flex items-center gap-3 p-3 rounded-lg border border-base-content/10 bg-base-200/50">
      <span class="flex-1 min-w-0 text-sm font-medium truncate">{$botName || '…'}</span>
      {#if $botRenameUrl}
        <button class="btn btn-sm btn-outline shrink-0" onclick={() => openBotRename($botRenameUrl)}>{$t('settingsAccount.renameBot')}</button>
      {/if}
    </div>
    {#if primaryOffer}
      <div class="flex flex-wrap items-center gap-2 rounded-lg border border-base-300 bg-base-200/40 px-3 py-2.5 mt-3">
        <span class="flex-1 min-w-0 text-sm">{$t('settingsAccount.alsoRenamePrimary', { values: { name: primaryOffer } })}</span>
        <button type="button" class="btn btn-sm btn-primary" onclick={renamePrimary} disabled={primaryBusy}>{$t('settingsAccount.renamePrimary')}</button>
        <button type="button" class="btn btn-sm btn-ghost" onclick={() => (primaryOffer = null)} disabled={primaryBusy}>{$t('settingsAccount.keepPrimaryName', { values: { name: primaryName } })}</button>
        {#if primaryError}<div class="w-full text-xs text-error">{primaryError}</div>{/if}
      </div>
    {:else if primaryRenamedTo}
      <div class="text-xs text-success mt-2">{$t('settingsAccount.primaryRenamed', { values: { name: primaryRenamedTo } })}</div>
    {/if}
  {/if}
</div>

<!-- The bot's Location -->
<div class="mb-8">
  <h3 class="text-base font-semibold mb-1 flex items-center gap-2"><MapPin class="w-4 h-4 text-base-content/70" aria-hidden="true" />{$t('settingsAccount.botLocation')}</h3>
  <p class="text-xs text-base-content/70 mb-2.5">{$t('settingsAccount.botLocationDesc')}</p>
  <form class="flex items-center gap-2" onsubmit={(e) => { e.preventDefault(); saveLocation(locationDraft); }}>
    <input
      type="text"
      bind:value={locationDraft}
      placeholder={$t('settingsAccount.botLocationPlaceholder')}
      aria-label={$t('settingsAccount.botLocation')}
      maxlength="300"
      class="flex-1 min-w-0 py-2 px-3 rounded-lg border border-base-content/25 bg-base-200/40 text-sm outline-none focus:border-base-content/50"
    />
    <button type="submit" class="btn btn-sm btn-outline shrink-0" disabled={locationBusy || !locationDraft.trim() || locationDraft.trim() === (location?.label ?? '')}>{locationBusy ? $t('common.saving') : $t('common.save')}</button>
    {#if location}
      <button type="button" class="btn btn-sm btn-ghost shrink-0" disabled={locationBusy} onclick={() => saveLocation('')}>{$t('common.remove')}</button>
    {/if}
  </form>
  {#if locationError}
    <div class="text-xs text-error mt-2">{locationError}</div>
  {:else if locationSaved}
    <div class="text-xs text-success mt-2">{$t('common.saved')}</div>
  {:else if location && location.latitude === undefined}
    <div class="text-xs text-base-content/70 mt-2">{$t('settingsAccount.botLocationPending')}</div>
  {/if}
</div>

<!-- Bot Identity (immutable) -->
<div class="mb-8">
  <h3 class="text-base font-semibold mb-1">{$t('settingsAccount.botIdentity')}</h3>
  <p class="text-xs text-base-content/70 mb-2.5">{$t('settingsAccount.botIdentityDesc')}</p>
  <div class="flex items-center gap-3 p-3 rounded-lg border border-base-content/10 bg-base-200/50" data-selectable>
    <span class="font-mono text-sm font-medium text-base-content shrink-0">@{defaultHandle || 'bot_…'}</span>
    {#if botId}
      <span class="font-mono text-xs text-base-content/50 truncate">{botId}</span>
    {/if}
  </div>
</div>

{#if botEmail}
  <!-- The bot's own email address -->
  <div class="mb-8">
    <h3 class="text-base font-semibold mb-1">{$t('settingsAccount.botEmail')}</h3>
    <p class="text-xs text-base-content/70 mb-2.5">{$t('settingsAccount.botEmailDesc')}</p>
    <div class="flex items-center gap-3 p-3 rounded-lg border border-base-content/10 bg-base-200/50" data-selectable>
      <span class="font-mono text-sm font-medium text-base-content truncate">{botEmail}</span>
    </div>
  </div>
{/if}

<!-- Danger zone -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3 text-error">{$t('settingsAccount.dangerZone')}</h3>
  <div class="p-4 rounded-xl border border-error/20 bg-base-100">
    <div class="flex items-center justify-between">
      <div>
        <div class="text-sm font-medium">{$t('settingsAccount.deleteModal.title')}</div>
        <div class="text-sm">{$t('settingsAccount.deleteAccountDesc')}</div>
      </div>
      <button class="px-3 py-1.5 rounded-lg border border-error/30 text-sm text-error font-medium cursor-pointer hover:bg-error/5 transition-colors" onclick={handleDeleteAccount}>{$t('settingsAccount.deleteModal.title')}</button>
    </div>
  </div>
</div>
