<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { neboAIOAuthStartWithJanus, neboAIOAuthStatus } from '$lib/api/index';

  let user = $state({ name: '', email: '', displayName: '' });
  let connected = $state(true);
  let reconnecting = $state(false);
  let reconnectError = $state('');
  let oauthPollInterval: ReturnType<typeof setInterval> | null = null;
  let oauthTimeout: ReturnType<typeof setTimeout> | null = null;

  // The bot's immutable, globally-unique id (full UUID) — shown read-only as the
  // bot's permanent identity. Never changes; independent of name and handle.
  let botId = $state('');

  // The bot's default id-based handle (`bot_<id8>`), independent of the display
  // name. Shown read-only as the bot's permanent identity.
  let defaultHandle = $state('');

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
  });

  async function reconnect() {
    reconnecting = true;
    reconnectError = '';
    try {
      const result = await neboAIOAuthStartWithJanus(false);
      const pendingState = result.state;

      oauthTimeout = setTimeout(() => {
        if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
        reconnecting = false;
        reconnectError = $t('settingsAccount.connectionTimeout');
      }, 180_000);

      oauthPollInterval = setInterval(async () => {
        try {
          const status = await neboAIOAuthStatus(pendingState);
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
</div>

<div class="mb-8">
  <a href="/settings/usage" class="text-sm font-medium text-primary hover:underline">{$t('settingsAccount.viewUsageArrow')}</a>
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
