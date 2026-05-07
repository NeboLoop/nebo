<script lang="ts">
  import { onMount, onDestroy } from 'svelte';

  let plugins = $state<{ id: string; name: string; desc: string; hasAuth: boolean; type: string }[]>([]);
  let authStatuses = $state<Record<string, 'connected' | 'disconnected' | 'connecting'>>({});

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listPlugins();
      if (resp?.plugins?.length) {
        plugins = (resp.plugins as Record<string, unknown>[]).map((p) => ({
          id: String(p.id || p.slug || ''),
          name: String(p.name || ''),
          desc: String(p.description || ''),
          hasAuth: !!(p.hasAuth ?? p.has_auth ?? false),
          type: 'plugin' as const,
        }));

        // Fetch actual auth status for each plugin that has auth
        for (const plugin of plugins) {
          if (!plugin.hasAuth) continue;
          try {
            const status = await api.pluginAuthStatus(plugin.id) as any;
            authStatuses[plugin.id] = status?.authenticated ? 'connected' : 'disconnected';
          } catch {
            authStatuses[plugin.id] = 'disconnected';
          }
        }
      }
    } catch {}

    // Listen for WS auth events
    window.addEventListener('nebo:plugin_auth_complete', handleAuthComplete as EventListener);
    window.addEventListener('nebo:plugin_auth_error', handleAuthError as EventListener);
    window.addEventListener('nebo:plugin_auth_url', handleAuthUrl as EventListener);
  });

  onDestroy(() => {
    window.removeEventListener('nebo:plugin_auth_complete', handleAuthComplete as EventListener);
    window.removeEventListener('nebo:plugin_auth_error', handleAuthError as EventListener);
    window.removeEventListener('nebo:plugin_auth_url', handleAuthUrl as EventListener);
  });

  function handleAuthComplete(e: CustomEvent) {
    const plugin = e.detail?.plugin;
    if (plugin) authStatuses[plugin] = 'connected';
  }

  function handleAuthError(e: CustomEvent) {
    const plugin = e.detail?.plugin;
    if (plugin) authStatuses[plugin] = 'disconnected';
  }

  function handleAuthUrl(e: CustomEvent) {
    const url = e.detail?.url;
    if (url) window.open(url, '_blank');
  }

  const installedPlugins = $derived(plugins);

  async function connectPlugin(id: string) {
    authStatuses[id] = 'connecting';
    try {
      const api = await import('$lib/api/nebo');
      await api.pluginAuthLogin(id);
      // Don't set connected here — wait for plugin_auth_complete WS event
    } catch {
      authStatuses[id] = 'disconnected';
    }
  }

  async function disconnectPlugin(id: string) {
    authStatuses[id] = 'disconnected';
    try {
      const api = await import('$lib/api/nebo');
      await api.pluginAuthLogout(id);
    } catch { /* local state already updated */ }
  }
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Plugins</h2>
  <p class="text-xs text-base-content/70">Manage installed plugins and their connections.</p>
</div>

{#if installedPlugins.length === 0}
  <div class="text-center py-12">
    <div class="text-xs text-base-content/50 mb-2">No plugins installed.</div>
    <a href="/marketplace/plugins" class="text-sm text-primary hover:underline">Browse plugins &rarr;</a>
  </div>
{:else}
  <div class="flex flex-col gap-2">
    {#each installedPlugins as plugin}
      <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
        <div class="w-9 h-9 rounded-lg bg-base-200 grid place-items-center text-base shrink-0">&#128268;</div>
        <div class="flex-1">
          <div class="text-sm font-semibold mb-0.5">{plugin.name}</div>
          <div class="text-xs text-base-content/50">{plugin.desc}</div>
        </div>
        {#if plugin.hasAuth}
          {@const status = authStatuses[plugin.id] ?? 'disconnected'}
          {#if status === 'connected'}
            <span class="px-2 py-0.5 rounded text-sm font-medium bg-success/10 text-success">Connected</span>
            <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={() => disconnectPlugin(plugin.id)}>Disconnect</button>
          {:else if status === 'connecting'}
            <span class="px-2 py-0.5 rounded text-sm font-medium bg-info/10 text-info">Connecting...</span>
          {:else}
            <button class="px-3 py-1 rounded-md border border-primary/30 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => connectPlugin(plugin.id)}>Connect</button>
          {/if}
        {:else}
          <span class="text-sm text-base-content/40">No auth needed</span>
        {/if}
      </div>
    {/each}
  </div>
{/if}
