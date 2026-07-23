<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';

  let presets = $state<{ id: string; name: string }[]>([]);
  let selectedPreset = $state('professional');

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [presetsResp, personalityResp] = await Promise.all([
        api.listPersonalityPresets(),
        api.getPersonality(),
      ]);
      const presetList = presetsResp?.presets as Record<string, unknown>[] | undefined;
      if (presetList?.length) {
        presets = presetList.map((p) => ({
          id: String(p.id || (p.name as string)?.toLowerCase() || ''),
          name: String(p.name || ''),
        }));
      }
      const personality = personalityResp as Record<string, unknown> | null;
      if (personality?.personalityPreset) {
        selectedPreset = String(personality.personalityPreset);
      }
    } catch { /* keep mock data */ }
  });

  async function savePersonality() {
    try {
      const api = await import('$lib/api/nebo');
      await api.updatePersonality({ personalityPreset: selectedPreset });
    } catch { /* ignore */ }
  }
  const dimensions = $derived([
    { id: 'voice', label: $t('settingsPersonality.voice'), options: ['neutral', 'warm', 'professional', 'enthusiastic'], value: 'professional' },
    { id: 'length', label: $t('settingsPersonality.responseLength'), options: ['concise', 'adaptive', 'detailed'], value: 'adaptive' },
    { id: 'emoji', label: $t('settingsPersonality.emojiUsage'), options: ['none', 'minimal', 'moderate', 'frequent'], value: 'minimal' },
    { id: 'formality', label: $t('settingsPersonality.formality'), options: ['casual', 'adaptive', 'formal'], value: 'adaptive' },
    { id: 'proactivity', label: $t('settingsPersonality.proactivity'), options: ['reactive', 'moderate', 'proactive'], value: 'moderate' },
  ]);
</script>

<SettingsHeader title={$t('settingsPersonality.pageTitle')} description={$t('settingsPersonality.pageDescription')} />

<!-- Preset selector -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-1.5">{$t('settingsPersonality.preset')}</div>
  <div class="flex gap-1.5 flex-wrap">
    {#each presets as preset}
      <button class="px-3.5 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {selectedPreset === preset.id
        ? 'bg-primary/10 text-primary border-primary font-medium'
        : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
        onclick={() => selectedPreset = preset.id}>
        {preset.name}
      </button>
    {/each}
  </div>
</div>

<!-- System prompt -->
<label class="block mb-8">
  <span class="block text-sm font-semibold mb-1.5">{$t('settingsPersonality.customSystemPrompt')}</span>
  <textarea rows="4" class="w-full py-2.5 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y font-mono leading-relaxed" placeholder={$t('settingsPersonality.customPromptPlaceholder')}>You are a professional AI employee. Be clear, concise, and business-focused. Prioritize accuracy and actionable insights.</textarea>
</label>

<!-- Tuning dimensions -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-4">{$t('settingsPersonality.tuning')}</h3>

  {#each dimensions as dim}
    <div class="mb-5">
      <div class="flex items-center justify-between mb-1.5">
        <span class="text-sm font-semibold">{dim.label}</span>
        <span class="text-sm font-mono">{$t('settingsPersonality.options.' + dim.value)}</span>
      </div>
      <div class="flex gap-1.5">
        {#each dim.options as opt}
          <button class="flex-1 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {dim.value === opt
            ? 'bg-primary/10 text-primary border-primary font-medium'
            : 'border-base-content/10 bg-base-100 hover:bg-base-200'}">{$t('settingsPersonality.options.' + opt)}</button>
        {/each}
      </div>
    </div>
  {/each}
</div>

<div class="flex gap-2">
  <button class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer hover:opacity-90 transition-opacity" onclick={savePersonality}>{$t('common.saveChanges')}</button>
  <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">{$t('settingsPersonality.revertToDefault')}</button>
</div>
