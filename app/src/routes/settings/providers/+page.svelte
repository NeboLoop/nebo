<script lang="ts">
  import { onMount } from 'svelte';
  import KeyIcon from 'lucide-svelte/icons/key';
  import type { AuthProfile } from '$lib/api/nebo';

  let providers = $state<{ id: string; name: string; provider: string; model: string; status: string; keySet: boolean }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAuthProfiles();
      if (resp?.profiles?.length) {
        providers = resp.profiles.map((p: AuthProfile) => ({
          id: p.id,
          name: p.name || p.provider,
          provider: p.provider || '',
          model: p.model || '',
          status: p.isActive ? 'connected' : 'disconnected',
          keySet: !!p.isActive,
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Providers</h2>
  <p class="text-xs text-base-content/70">Configure LLM providers and API keys.</p>
</div>

<div class="flex flex-col gap-2">
  {#each providers as provider}
    <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="w-9 h-9 rounded-lg bg-base-200 grid place-items-center shrink-0 text-base-content/50"><KeyIcon class="w-4 h-4" /></div>
      <div class="flex-1">
        <div class="text-sm font-semibold mb-0.5">{provider.name}</div>
        <div class="text-sm text-base-content/60">{provider.model || provider.provider}</div>
      </div>
      <span class="px-2 py-0.5 rounded text-sm font-semibold {provider.status === 'connected'
        ? 'bg-success/10 text-success'
        : 'bg-base-200'}">
        {provider.status === 'connected' ? 'Connected' : 'Not connected'}
      </span>
      <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">
        {provider.keySet ? 'Edit Key' : 'Add Key'}
      </button>
    </div>
  {/each}
</div>
