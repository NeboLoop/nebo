<script lang="ts">
  import { onMount } from 'svelte';

  let user = $state({ name: '', email: '', displayName: '' });
  let connected = $state(true);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getProfile() as { profile?: Record<string, unknown> };
      if (resp?.profile) {
        const p = resp.profile;
        user = {
          ...user,
          name: String(p.displayName || p.name || user.name),
          displayName: String(p.displayName || p.name || user.displayName),
          email: user.email,
        };
      }
      // Try to get email from current user
      const userResp = await api.userGetCurrentUser().catch(() => null);
      if (userResp) {
        user.email = userResp.email || user.email;
        user.name = user.name || userResp.name;
      }
      const status = await api.neboLoopAccountStatus() as { connected?: boolean } | null;
      if (status) {
        connected = !!status.connected;
      }
    } catch { /* keep mock data */ }
  });

  async function disconnect() {
    try {
      const api = await import('$lib/api/nebo');
      await api.neboLoopAccountDisconnect();
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
  <h2 class="text-lg font-bold mb-1">NeboLoop Account</h2>
  <p class="text-xs text-base-content/70">Manage your NeboLoop connection and account settings.</p>
</div>

<!-- Connection status -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-4">
  <div class="flex items-center gap-3">
    <div class="w-10 h-10 rounded-lg bg-primary/20 text-primary grid place-items-center font-mono text-sm font-semibold">{user.name.charAt(0)}</div>
    <div class="flex-1">
      <div class="text-sm font-semibold">{user.displayName}</div>
      <div class="text-sm">{user.email}</div>
    </div>
    <span class="px-2 py-0.5 rounded text-sm font-semibold {connected ? 'bg-success/10 text-success' : 'bg-base-200'}">
      {connected ? 'Connected' : 'Disconnected'}
    </span>
  </div>
</div>

<div class="flex gap-2 mb-8">
  <a href="/settings/usage" class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium hover:bg-base-200 transition-colors">View Usage →</a>
  <button class="px-4 py-2 rounded-lg border border-error/20 text-sm font-medium text-error hover:bg-error/5 transition-colors cursor-pointer" onclick={disconnect}>Disconnect Account</button>
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
