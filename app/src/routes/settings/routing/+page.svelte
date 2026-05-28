<script lang="ts">
  import { onMount } from 'svelte';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import Brain from 'lucide-svelte/icons/brain';
  import Code from 'lucide-svelte/icons/code';
  import Eye from 'lucide-svelte/icons/eye';
  import Volume2 from 'lucide-svelte/icons/volume-2';
  import Tag from 'lucide-svelte/icons/tag';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';
  import type { AuthProfile } from '$lib/api/neboComponents';

  let loading = $state(true);
  let error = $state('');
  let providers = $state<AuthProfile[]>([]);
  let models = $state<Record<string, any[]>>({});
  let availableCLIs = $state<any>(null);
  let janusStatus = $state<any>(null);
  let cliProviderInfo = $state<Record<string, { id: string; name: string; command: string; models: string[] }>>({});
  let saving = $state(false);

  // Routing form state
  let routingForm = $state({ vision: 'auto', audio: 'auto', reasoning: 'auto', code: 'auto', general: 'auto' });
  let backupForm = $state({ vision: 'none', audio: 'none', reasoning: 'none', code: 'none', general: 'none' });
  let aliasesForm = $state<{ alias: string; modelId: string }[]>([]);
  let laneRoutingForm = $state({ heartbeat: 'auto', events: 'auto', comm: 'auto', subagent: 'auto' });

  const janusCoveredProviders = ['anthropic', 'openai', 'google', 'deepseek'];

  const providerOptions = [
    { value: 'anthropic', label: 'Anthropic (Claude)' },
    { value: 'openai', label: 'OpenAI (GPT)' },
    { value: 'google', label: 'Google (Gemini)' },
    { value: 'deepseek', label: 'DeepSeek' },
    { value: 'ollama', label: 'Ollama (Local)' },
  ];

  type TaskKey = 'general' | 'reasoning' | 'code' | 'vision' | 'audio';
  type LaneKey = 'heartbeat' | 'events' | 'comm' | 'subagent';

  const routingModes: { key: TaskKey; label: string; description: string; icon: any; color: string }[] = [
    { key: 'general', label: 'All-Purpose', description: 'General tasks and conversation', icon: Sparkles, color: 'text-primary' },
    { key: 'reasoning', label: 'Reasoning', description: 'Complex analysis and logic', icon: Brain, color: 'text-secondary' },
    { key: 'code', label: 'Advanced', description: 'Code generation and execution', icon: Code, color: 'text-accent' },
    { key: 'vision', label: 'Vision', description: 'Image understanding', icon: Eye, color: 'text-info' },
    { key: 'audio', label: 'Audio', description: 'Voice and audio processing', icon: Volume2, color: 'text-warning' },
  ];

  const laneModes: { key: LaneKey; label: string; description: string }[] = [
    { key: 'heartbeat', label: 'Heartbeat', description: 'Health monitoring operations' },
    { key: 'events', label: 'Scheduled', description: 'Scheduled and event operations' },
    { key: 'comm', label: 'Communication', description: 'Messaging operations' },
    { key: 'subagent', label: 'Sub-agents', description: 'Sub-agent delegation' },
  ];

  // Debounced auto-save
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleAutoSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => persist(), 400);
  }

  async function persist() {
    if (saving) return;
    saving = true;
    error = '';
    try {
      const api = await import('$lib/api/nebo');
      const toApi = (v: string) => (v === 'auto' || v === 'none') ? '' : v;
      const fallbacks: Record<string, string[]> = {};
      for (const mode of routingModes) {
        const backup = toApi(backupForm[mode.key]);
        if (backup) fallbacks[mode.key] = [backup];
      }

      const validAliases = aliasesForm.filter(a => a.alias.trim() && a.modelId);

      const laneRouting: Record<string, string> = {};
      for (const lane of laneModes) {
        if (laneRoutingForm[lane.key]) laneRouting[lane.key] = laneRoutingForm[lane.key];
      }

      await api.updateTaskRouting({
        vision: toApi(routingForm.vision),
        audio: toApi(routingForm.audio),
        reasoning: toApi(routingForm.reasoning),
        code: toApi(routingForm.code),
        general: toApi(routingForm.general),
        fallbacks,
        aliases: validAliases,
        laneRouting: Object.keys(laneRouting).length > 0 ? laneRouting : undefined,
      });
    } catch (err: any) {
      error = err?.message || 'Failed to save routing';
    } finally { saving = false; }
  }

  onMount(loadData);

  async function loadData() {
    loading = true;
    error = '';
    try {
      const api = await import('$lib/api/nebo');
      const [modelsRes, profilesRes, janusRes] = await Promise.all([
        api.listModels(),
        api.listProviders(),
        api.neboAIAccountStatus().catch(() => null),
      ]);

      models = (modelsRes.models as Record<string, any[]>) || {};
      providers = profilesRes.profiles || [];
      janusStatus = janusRes;
      availableCLIs = modelsRes.availableCLIs || null;

      // CLI provider info
      if (modelsRes.cliProviders) {
        const info: Record<string, any> = {};
        for (const cp of modelsRes.cliProviders as any[]) {
          info[cp.command] = { id: cp.id, name: cp.displayName, command: cp.command, models: cp.models || [] };
        }
        cliProviderInfo = info;
      }

      // Populate task routing form
      const taskRouting = modelsRes.taskRouting as Record<string, any> | undefined;
      if (taskRouting) {
        const validValues = new Set(getGroupedModelOptions().flatMap(g => g.models.map((m: any) => m.value)));
        const norm = (v: string | undefined) => (v && validValues.has(v)) ? v : 'auto';
        const normB = (v: string | undefined) => (v && validValues.has(v)) ? v : 'none';

        routingForm = {
          vision: norm(taskRouting.vision),
          audio: norm(taskRouting.audio),
          reasoning: norm(taskRouting.reasoning),
          code: norm(taskRouting.code),
          general: norm(taskRouting.general),
        };
        const fb = taskRouting.fallbacks || {};
        backupForm = {
          vision: normB(fb['vision']?.[0]),
          audio: normB(fb['audio']?.[0]),
          reasoning: normB(fb['reasoning']?.[0]),
          code: normB(fb['code']?.[0]),
          general: normB(fb['general']?.[0]),
        };
      }

      // Aliases
      aliasesForm = ((modelsRes.aliases || []) as any[]).map(a => ({ alias: a.alias, modelId: a.modelId }));

      // Lane routing
      const lr = modelsRes.laneRouting as Record<string, string> | undefined;
      if (lr) {
        laneRoutingForm = {
          heartbeat: lr['heartbeat'] || 'auto',
          events: lr['events'] || 'auto',
          comm: lr['comm'] || 'auto',
          subagent: lr['subagent'] || 'auto',
        };
      }
    } catch (err: any) {
      error = err?.message || 'Failed to load routing config';
    } finally { loading = false; }
  }

  function getGroupedModelOptions(): { provider: string; label: string; models: { value: string; label: string }[] }[] {
    const groups: { provider: string; label: string; models: { value: string; label: string }[] }[] = [];
    const configuredProviders = new Set(providers.filter(p => p.isActive).map(p => p.provider));
    const janusConnected = janusStatus?.connected && janusStatus.janusProvider;
    const cliIds = new Set(Object.values(cliProviderInfo).map(c => c.id));

    // Janus first
    if (models['janus']) {
      const active = models['janus'].filter((m: any) => m.isActive);
      if (active.length > 0) {
        groups.push({
          provider: 'janus', label: 'Janus (NeboAI)',
          models: active.map((m: any) => ({ value: `janus/${m.id}`, label: m.displayName })),
        });
      }
    }

    // API providers
    for (const [providerType, modelList] of Object.entries(models)) {
      if (providerType === 'janus') continue;
      if (cliIds.has(providerType)) continue;
      const hasKey = configuredProviders.has(providerType);
      const coveredByJanus = janusConnected && janusCoveredProviders.includes(providerType);
      if (!hasKey && !coveredByJanus) continue;
      const active = modelList.filter((m: any) => m.isActive);
      if (active.length === 0) continue;
      const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
      groups.push({
        provider: providerType, label,
        models: active.map((m: any) => ({ value: `${providerType}/${m.id}`, label: m.displayName })),
      });
    }

    // CLI providers
    for (const cli of Object.values(cliProviderInfo)) {
      const isAvailable =
        (cli.command === 'claude' && (availableCLIs as any)?.claude) ||
        (cli.command === 'codex' && (availableCLIs as any)?.codex) ||
        (cli.command === 'gemini' && (availableCLIs as any)?.gemini);
      if (!isAvailable) continue;
      if (models[cli.id]?.length) continue;
      groups.push({
        provider: cli.id, label: cli.name,
        models: cli.models.map(mid => ({ value: `${cli.id}/${mid}`, label: mid })),
      });
    }

    return groups;
  }

  const groups = $derived(getGroupedModelOptions());

  function addAlias() {
    aliasesForm = [...aliasesForm, { alias: '', modelId: '' }];
  }

  function removeAlias(index: number) {
    aliasesForm = aliasesForm.filter((_, i) => i !== index);
    scheduleAutoSave();
  }
</script>

<div class="mb-7">
  <h2 class="text-base font-semibold mb-1">Routing</h2>
  <p class="text-xs text-base-content/70">Configure which models handle which task types.</p>
</div>

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">Loading routing config...</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    <!-- Task Routing -->
    <section>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Task Routing</div>
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
        <div class="flex flex-col gap-5">
          {#each routingModes as mode}
            <div>
              <div class="flex items-center gap-2 mb-2">
                <svelte:component this={mode.icon} class="w-3.5 h-3.5 {mode.color} shrink-0" />
                <span class="text-sm font-medium">{mode.label}</span>
                <span class="text-xs text-base-content/50">{mode.description}</span>
              </div>
              <div class="grid sm:grid-cols-2 gap-2">
                <div>
                  <label class="text-xs text-base-content/50 mb-1 block">Primary</label>
                  <select bind:value={routingForm[mode.key]} onchange={scheduleAutoSave} class="select select-bordered select-sm w-full">
                    <option value="auto">Auto</option>
                    {#each groups as group}
                      <optgroup label={group.label}>
                        {#each group.models as opt}
                          <option value={opt.value}>{opt.label}</option>
                        {/each}
                      </optgroup>
                    {/each}
                  </select>
                </div>
                <div>
                  <label class="text-xs text-base-content/50 mb-1 block">Backup</label>
                  <select bind:value={backupForm[mode.key]} onchange={scheduleAutoSave} class="select select-bordered select-sm w-full">
                    <option value="none">None</option>
                    {#each groups as group}
                      <optgroup label={group.label}>
                        {#each group.models as opt}
                          <option value={opt.value}>{opt.label}</option>
                        {/each}
                      </optgroup>
                    {/each}
                  </select>
                </div>
              </div>
            </div>
          {/each}
        </div>

        <!-- Custom Aliases -->
        {#if aliasesForm.filter(a => !['claude', 'codex', 'gemini'].includes(a.alias)).length > 0}
          <div class="mt-5 pt-5 border-t border-base-content/10">
            <div class="flex items-center gap-2 mb-3">
              <Tag class="w-3 h-3 text-base-content/50" />
              <span class="text-xs font-medium text-base-content/70">Custom Aliases</span>
            </div>
            <div class="flex flex-col gap-2">
              {#each aliasesForm as aliasEntry, index}
                {#if !['claude', 'codex', 'gemini'].includes(aliasEntry.alias)}
                  <div class="flex items-center gap-2">
                    <input
                      type="text"
                      placeholder="Alias name"
                      bind:value={aliasEntry.alias}
                      onblur={scheduleAutoSave}
                      class="input input-bordered input-sm w-36"
                    />
                    <select bind:value={aliasEntry.modelId} onchange={scheduleAutoSave} class="select select-bordered select-sm flex-1">
                      <option value="">Select model...</option>
                      {#each groups as group}
                        <optgroup label={group.label}>
                          {#each group.models as opt}
                            <option value={opt.value}>{opt.label}</option>
                          {/each}
                        </optgroup>
                      {/each}
                    </select>
                    <button
                      type="button"
                      class="btn btn-ghost btn-sm btn-square"
                      onclick={() => removeAlias(index)}
                    >
                      <Trash2 class="w-3.5 h-3.5 text-base-content/50" />
                    </button>
                  </div>
                {/if}
              {/each}
            </div>
          </div>
        {/if}

        <div class="mt-3">
          <button
            type="button"
            class="flex items-center gap-1.5 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
            onclick={addAlias}
          >
            <Plus class="w-3.5 h-3.5" /> Add Shortcut
          </button>
        </div>
      </div>
    </section>

    <!-- Lane Routing -->
    <section>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Lane Routing</div>
      <p class="text-xs text-base-content/50 mb-2">Assign models to background operation lanes.</p>
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
        <div class="flex flex-col gap-5">
          {#each laneModes as lane}
            <div>
              <div class="flex items-center gap-2 mb-2">
                <span class="text-sm font-medium">{lane.label}</span>
                <span class="text-xs text-base-content/50">{lane.description}</span>
              </div>
              <select bind:value={laneRoutingForm[lane.key]} onchange={scheduleAutoSave} class="select select-bordered select-sm w-full">
                <option value="auto">Auto</option>
                {#each groups as group}
                  <optgroup label={group.label}>
                    {#each group.models as opt}
                      <option value={opt.value}>{opt.label}</option>
                    {/each}
                  </optgroup>
                {/each}
              </select>
            </div>
          {/each}
        </div>
      </div>
    </section>
  </div>
{/if}
