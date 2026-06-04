<script lang="ts">
  import { onMount } from 'svelte';
  import Lock from 'lucide-svelte/icons/lock';
  import Zap from 'lucide-svelte/icons/zap';
  import Puzzle from 'lucide-svelte/icons/puzzle';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import CheckCircle from 'lucide-svelte/icons/check-circle';
  import AlertCircle from 'lucide-svelte/icons/alert-circle';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';

  interface SecretField {
    key: string;
    label: string;
    hint: string;
    required: boolean;
    configured: boolean;
  }

  interface SecretGroup {
    name: string;
    source: 'skill' | 'plugin';
    slug: string; // skill name or plugin slug for API calls
    secrets: SecretField[];
  }

  let loading = $state(true);
  let error = $state('');
  let groups = $state<SecretGroup[]>([]);
  let settingSecret = $state<string | null>(null);
  let secretInputs = $state<Record<string, string>>({});
  let successMsg = $state('');

  onMount(loadSecrets);

  async function loadSecrets() {
    loading = true;
    error = '';
    try {
      const api = await import('$lib/api/nebo');
      const loaded: SecretGroup[] = [];

      // 1. Skills (from listExtensions)
      try {
        const extResp = await api.listExtensions();
        for (const skill of extResp.extensions) {
          if (skill.secrets.length > 0) {
            loaded.push({
              name: skill.name,
              source: 'skill',
              slug: skill.name,
              secrets: skill.secrets,
            });
          }
        }
      } catch { /* no skills */ }

      // 2. Plugins (from listPlugins + getPluginConfig)
      try {
        const plugResp = await api.listPlugins();
        const plugins = (plugResp?.plugins ?? []) as Array<Record<string, unknown>>;
        for (const p of plugins) {
          const slug = String(p.id || p.slug || '');
          if (!slug) continue;
          try {
            const configResp = await api.getPluginConfig(slug) as { config?: SecretField[] };
            const fields = configResp?.config ?? [];
            const secretFields = fields.filter(f => f.secret || f.required);
            if (secretFields.length > 0) {
              loaded.push({
                name: String(p.name || slug),
                source: 'plugin',
                slug,
                secrets: secretFields.map(f => ({
                  key: f.key,
                  label: f.label || f.key,
                  hint: f.hint || (f as any).description || '',
                  required: f.required ?? false,
                  configured: !!(f as any).value && (f as any).value !== '',
                })),
              });
            }
          } catch { /* skip plugins without config */ }
        }
      } catch { /* no plugins */ }

      groups = loaded;
    } catch (err: any) {
      error = err?.message || 'Failed to load secrets';
    } finally { loading = false; }
  }

  async function saveSecret(group: SecretGroup, key: string) {
    const inputKey = `${group.slug}:${key}`;
    const value = secretInputs[inputKey];
    if (!value) return;
    settingSecret = inputKey;
    successMsg = '';
    try {
      const api = await import('$lib/api/nebo');
      const { setPluginConfig } = await import('$lib/api/index');
      if (group.source === 'skill') {
        await api.setSkillSecret(group.slug, { key, value });
      } else {
        await setPluginConfig(group.slug, { [key]: value.trim() });
      }
      secretInputs[inputKey] = '';
      successMsg = `Saved ${key} for ${group.name}`;
      await loadSecrets();
      setTimeout(() => successMsg = '', 3000);
    } catch (err: any) {
      error = err?.message || 'Failed to save secret';
    } finally { settingSecret = null; }
  }

  async function removeSecret(group: SecretGroup, key: string) {
    const inputKey = `${group.slug}:${key}`;
    settingSecret = inputKey;
    try {
      const api = await import('$lib/api/nebo');
      if (group.source === 'skill') {
        await api.deleteSkillSecret(group.slug, key);
      } else {
        const { setPluginConfig } = await import('$lib/api/index');
        await setPluginConfig(group.slug, { [key]: '' });
      }
      await loadSecrets();
    } catch (err: any) {
      error = err?.message || 'Failed to remove secret';
    } finally { settingSecret = null; }
  }
</script>

<div class="mb-7">
  <h2 class="text-base font-semibold mb-1">Secrets</h2>
  <p class="text-xs text-base-content/70">Manage API keys and credentials used by your skills and plugins.</p>
</div>

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">Loading secrets...</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    {#if successMsg}
      <Alert type="success">{successMsg}</Alert>
    {/if}

    {#if groups.length === 0}
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
        <div class="py-8 text-center">
          <Lock class="w-8 h-8 mx-auto mb-3 text-base-content/30" />
          <p class="text-sm font-medium text-base-content/70 mb-1">No secrets to configure</p>
          <p class="text-xs text-base-content/50 mb-4">Install skills or plugins from the marketplace that require API keys.</p>
          <a href="/marketplace" class="btn btn-primary btn-sm">Browse Marketplace</a>
        </div>
      </div>
    {:else}
      {#each groups as group (group.slug)}
        <section>
          <div class="flex items-center gap-2 mb-2">
            {#if group.source === 'plugin'}
              <Puzzle class="w-3.5 h-3.5 text-secondary" />
            {:else}
              <Zap class="w-3.5 h-3.5 text-primary" />
            {/if}
            <span class="text-sm font-medium">{group.name}</span>
            <span class="text-xs text-base-content/40">{group.source}</span>
          </div>
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4 flex flex-col gap-3">
            {#each group.secrets as secret (secret.key)}
              <div class="flex items-start gap-3 py-1">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2 mb-0.5">
                    <span class="text-sm font-medium">{secret.label || secret.key}</span>
                    {#if secret.required}
                      <span class="text-xs text-error/80">Required</span>
                    {/if}
                    {#if secret.configured}
                      <CheckCircle class="w-3.5 h-3.5 text-success" />
                    {:else}
                      <AlertCircle class="w-3.5 h-3.5 text-warning" />
                    {/if}
                  </div>
                  {#if secret.hint}
                    <p class="text-xs text-base-content/50">{secret.hint}</p>
                  {/if}

                  {#if secret.configured}
                    <div class="flex items-center gap-2 mt-2">
                      <span class="text-xs text-success/80">Configured</span>
                      <button
                        type="button"
                        class="text-base-content/30 hover:text-error transition-colors cursor-pointer"
                        onclick={() => removeSecret(group, secret.key)}
                        disabled={settingSecret === `${group.slug}:${secret.key}`}
                      >
                        <Trash2 class="w-3.5 h-3.5" />
                      </button>
                    </div>
                  {:else}
                    <div class="flex gap-2 mt-2">
                      <input
                        type="password"
                        placeholder={secret.key}
                        bind:value={secretInputs[`${group.slug}:${secret.key}`]}
                        class="input input-bordered input-sm flex-1 font-mono"
                        onkeydown={(e) => { if (e.key === 'Enter') saveSecret(group, secret.key); }}
                      />
                      <button
                        type="button"
                        class="btn btn-primary btn-sm"
                        onclick={() => saveSecret(group, secret.key)}
                        disabled={settingSecret === `${group.slug}:${secret.key}` || !secretInputs[`${group.slug}:${secret.key}`]}
                      >
                        {settingSecret === `${group.slug}:${secret.key}` ? '...' : 'Save'}
                      </button>
                    </div>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        </section>
      {/each}
    {/if}
  </div>
{/if}
