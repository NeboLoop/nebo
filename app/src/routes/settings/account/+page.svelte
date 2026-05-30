<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { neboAIOAuthStartWithJanus, neboAIOAuthStatus } from '$lib/api/index';
  import Check from 'lucide-svelte/icons/check';
  import X from 'lucide-svelte/icons/x';
  import LoaderCircle from 'lucide-svelte/icons/loader-circle';

  let user = $state({ name: '', email: '', displayName: '' });
  let connected = $state(true);
  let reconnecting = $state(false);
  let reconnectError = $state('');
  let oauthPollInterval: ReturnType<typeof setInterval> | null = null;
  let oauthTimeout: ReturnType<typeof setTimeout> | null = null;

  // --- Bot Handle (global, bot-level identity) ---
  // The handle is the bot's globally-unique identity on NeboAI/Loop, stored on
  // the primary "assistant" agent (which IS the bot — codes.rs reads the CONNECT
  // handle from there). The input edits only the `<chosen>` part; the `bot_`
  // prefix is a fixed affordance. Empty ⇒ the default `bot_<id8>` is used.
  let editHandle = $state('');
  let handleSaved = $state(false);
  let handleSaveTimer: ReturnType<typeof setTimeout> | null = null;

  // The bot's immutable, globally-unique id (full UUID) — shown read-only as the
  // bot's permanent identity. Never changes; independent of name and handle.
  let botId = $state('');

  // The bot's default id-based handle (`bot_<id8>`), independent of the display
  // name. Shown as the placeholder and used as the effective handle when blank.
  let defaultHandle = $state('');
  const defaultHandleSuffix = $derived(defaultHandle.replace(/^bot_/, '') || 'handle');

  // The handle currently stored (stripped of `bot_`), so we don't flag the bot's
  // own existing handle as taken.
  let currentHandleSuffix = $state('');

  type HandleAvail = 'idle' | 'checking' | 'available' | 'taken';
  let handleAvail = $state<HandleAvail>('idle');
  let handleCheckTimer: ReturnType<typeof setTimeout> | null = null;
  // Token guards against out-of-order responses clobbering a newer check.
  let handleCheckToken = 0;

  onDestroy(() => {
    if (oauthPollInterval) clearInterval(oauthPollInterval);
    if (oauthTimeout) clearTimeout(oauthTimeout);
    if (handleSaveTimer) clearTimeout(handleSaveTimer);
    if (handleCheckTimer) clearTimeout(handleCheckTimer);
  });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const status = await api.neboAIAccountStatus() as Record<string, unknown> | null;
      if (status) {
        connected = !!status.connected;
        if (status.email) user.email = String(status.email);
        if (status.displayName) {
          user.displayName = String(status.displayName);
          user.name = String(status.displayName);
        }
      }
    } catch { /* keep mock data */ }

    // Load the default `bot_<id8>` handle (placeholder) and the bot's current
    // stored handle (from the primary "assistant" agent) for the input.
    try {
      const api = await import('$lib/api/nebo');
      const botStatus = (await api.neboAIBotStatus()) as { defaultHandle?: string; botId?: string };
      if (botStatus?.defaultHandle) defaultHandle = botStatus.defaultHandle;
      if (botStatus?.botId) botId = botStatus.botId;
    } catch { /* not connected — placeholder falls back to generic 'handle' */ }
    try {
      const api = await import('$lib/api/nebo');
      const resp = (await api.getAgent('assistant')) as { handle?: string };
      currentHandleSuffix = (resp?.handle ?? '').replace(/^bot_/, '');
      editHandle = currentHandleSuffix;
    } catch { /* leave blank */ }
  });

  function debounceHandleCheck() {
    if (handleCheckTimer) clearTimeout(handleCheckTimer);
    const chosen = editHandle.trim();
    // Empty custom handle ⇒ the default bot_<id8> is used; nothing to check.
    // Unchanged from the stored handle ⇒ it's already ours.
    if (chosen === '' || chosen === currentHandleSuffix) {
      handleAvail = 'idle';
      return;
    }
    handleAvail = 'checking';
    const token = ++handleCheckToken;
    handleCheckTimer = setTimeout(async () => {
      try {
        const api = await import('$lib/api/nebo');
        const res = (await api.handleAvailable(`bot_${chosen}`)) as { available?: boolean };
        if (token !== handleCheckToken) return; // superseded by a newer keystroke
        handleAvail = res?.available ? 'available' : 'taken';
      } catch {
        if (token === handleCheckToken) handleAvail = 'idle';
      }
    }, 400);
  }

  function onHandleInput() {
    debounceHandleCheck();
    if (handleSaveTimer) clearTimeout(handleSaveTimer);
    handleSaveTimer = setTimeout(() => saveHandle(), 800);
  }

  async function saveHandle() {
    try {
      const api = await import('$lib/api/nebo');
      const chosen = editHandle.trim();
      // The primary "assistant" agent IS the bot; codes.rs reads the CONNECT
      // handle from it. Empty custom handle ⇒ save `bot_` so the backend strips
      // the prefix to empty and falls back to `bot_<id8>`.
      await api.updateAgent('assistant', { handle: `bot_${chosen}` });
      currentHandleSuffix = chosen;
      handleAvail = 'idle';
      handleSaved = true;
      setTimeout(() => handleSaved = false, 2000);
    } catch { /* silent */ }
  }

  async function reconnect() {
    reconnecting = true;
    reconnectError = '';
    try {
      const result = await neboAIOAuthStartWithJanus(false);
      const pendingState = result.state;

      oauthTimeout = setTimeout(() => {
        if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
        reconnecting = false;
        reconnectError = 'Connection timed out. Please try again.';
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
            reconnectError = status.error || 'OAuth failed. Please try again.';
          } else if (status?.status === 'expired') {
            if (oauthPollInterval) { clearInterval(oauthPollInterval); oauthPollInterval = null; }
            if (oauthTimeout) { clearTimeout(oauthTimeout); oauthTimeout = null; }
            reconnecting = false;
            reconnectError = 'OAuth session expired. Please try again.';
          }
        } catch {
          // Poll error — keep trying
        }
      }, 2000);
    } catch (err) {
      reconnecting = false;
      reconnectError = err instanceof Error ? err.message : 'Failed to start OAuth flow';
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
    if (!confirm('Are you sure you want to delete your account? This cannot be undone.')) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.userDeleteAccount();
    } catch { /* ignore */ }
  }
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">NeboAI Account</h2>
  <p class="text-xs text-base-content/70">Manage your NeboAI connection and account settings.</p>
</div>

<!-- Connection status + inline connect/disconnect action -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-2">
  <div class="flex items-center gap-3">
    <div class="w-10 h-10 rounded-lg bg-primary/20 text-primary grid place-items-center font-mono text-sm font-semibold">{user.name.charAt(0)}</div>
    <div class="flex-1 min-w-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium truncate">{user.displayName}</span>
        <span class="px-2 py-0.5 rounded text-xs font-semibold {connected ? 'bg-success/10 text-success' : 'bg-base-200 text-base-content/70'}">
          {connected ? 'Connected' : 'Disconnected'}
        </span>
      </div>
      <div class="text-xs text-base-content/70 truncate">{user.email}</div>
    </div>
    {#if connected}
      <button class="shrink-0 px-3 py-1.5 rounded-lg border border-error/20 text-sm font-medium text-error hover:bg-error/5 transition-colors cursor-pointer" onclick={disconnect}>Disconnect</button>
    {:else}
      <button
        class="shrink-0 px-3 py-1.5 rounded-lg border border-primary/30 text-sm font-medium text-primary hover:bg-primary/5 transition-colors cursor-pointer disabled:opacity-50"
        onclick={reconnect}
        disabled={reconnecting}
      >{reconnecting ? 'Connecting…' : 'Connect'}</button>
    {/if}
  </div>
  {#if reconnectError}
    <div class="text-xs text-error mt-2">{reconnectError}</div>
  {/if}
</div>

<div class="mb-8">
  <a href="/settings/usage" class="text-sm font-medium text-primary hover:underline">View Usage →</a>
</div>

<!-- Bot Identity (immutable) -->
<div class="mb-8">
  <h3 class="text-base font-semibold mb-1">Bot Identity</h3>
  <p class="text-xs text-base-content/70 mb-2.5">Your bot's permanent, globally-unique identity. This never changes.</p>
  <div class="flex items-center gap-3 p-3 rounded-lg border border-base-content/10 bg-base-200/50" data-selectable>
    <span class="font-mono text-sm font-medium text-base-content shrink-0">@{defaultHandle || 'bot_…'}</span>
    {#if botId}
      <span class="font-mono text-xs text-base-content/50 truncate">{botId}</span>
    {/if}
  </div>
</div>

<!-- Bot Handle -->
<div class="mb-8">
  <div class="flex items-center justify-between mb-1">
    <h3 class="text-base font-semibold">Bot Handle</h3>
    {#if handleSaved}
      <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
    {/if}
  </div>
  <p class="text-xs text-base-content/70 mb-2.5">Your bot's globally-unique handle. Leave blank to use <span class="font-mono">@bot_{defaultHandleSuffix}</span>.</p>
  <div class="flex items-stretch rounded-lg border bg-base-100 overflow-hidden focus-within:border-base-content/40 transition-colors {handleAvail === 'available' ? 'border-success' : handleAvail === 'taken' ? 'border-error' : 'border-base-content/10'}">
    <span class="flex items-center px-3 text-sm font-mono text-base-content/50 bg-base-200 border-r border-base-content/10 select-none">@bot_</span>
    <input type="text" bind:value={editHandle} oninput={onHandleInput} placeholder={defaultHandleSuffix} class="flex-1 py-2 px-3 text-sm bg-base-100 outline-none font-mono" />
    {#if handleAvail === 'checking'}
      <span class="flex items-center px-3 text-base-content/50"><LoaderCircle class="w-4 h-4 animate-spin" /></span>
    {:else if handleAvail === 'available'}
      <span class="flex items-center gap-1 px-3 text-xs text-success"><Check class="w-4 h-4" /> Available</span>
    {:else if handleAvail === 'taken'}
      <span class="flex items-center gap-1 px-3 text-xs text-error"><X class="w-4 h-4" /> Taken</span>
    {/if}
  </div>
</div>

<!-- Danger zone -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3 text-error">Danger Zone</h3>
  <div class="p-4 rounded-xl border border-error/20 bg-base-100">
    <div class="flex items-center justify-between">
      <div>
        <div class="text-sm font-medium">Delete Account</div>
        <div class="text-sm">Permanently delete your account and all data. This cannot be undone.</div>
      </div>
      <button class="px-3 py-1.5 rounded-lg border border-error/30 text-sm text-error font-medium cursor-pointer hover:bg-error/5 transition-colors" onclick={handleDeleteAccount}>Delete Account</button>
    </div>
  </div>
</div>
