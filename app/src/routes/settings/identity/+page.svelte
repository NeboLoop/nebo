<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import RotateCcw from 'lucide-svelte/icons/rotate-ccw';

  let agentName = $state('');
  let emoji = $state('');
  let role = $state('');
  let creature = $state('');
  let vibe = $state('');
  let saved = $state(false);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  // Snapshot for revert
  let snap = $state({ agentName: '', emoji: '', role: '', creature: '', vibe: '' });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getProfile() as { profile?: Record<string, unknown> };
      if (resp?.profile) {
        const p = resp.profile;
        agentName = String(p.name || '');
        emoji = String(p.emoji || '');
        role = String(p.role || '');
        creature = String(p.creature || '');
        vibe = String(p.vibe || '');
        snap = { agentName, emoji, role, creature, vibe };
      }
    } catch { /* keep defaults */ }
  });

  function debounceSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => persist(), 800);
  }

  function saveNow() {
    if (saveTimer) clearTimeout(saveTimer);
    persist();
  }

  async function persist() {
    try {
      const api = await import('$lib/api/nebo');
      await api.updateProfile({
        name: agentName || undefined,
        emoji: emoji || undefined,
        role: role || undefined,
        creature: creature || undefined,
        vibe: vibe || undefined,
      });
      snap = { agentName, emoji, role, creature, vibe };
      saved = true;
      setTimeout(() => saved = false, 2000);
    } catch { /* silent */ }
  }

  function revert() {
    agentName = snap.agentName;
    emoji = snap.emoji;
    role = snap.role;
    creature = snap.creature;
    vibe = snap.vibe;
  }
</script>

<SettingsHeader title={$t('settingsIdentity.title')} description={$t('settingsIdentity.pageDescription')}>
  {#snippet action()}
    {#if saved}
      <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> {$t('common.saved')}</span>
    {/if}
    <button onclick={revert} class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none" title={$t('common.revertToLastSaved')}>
      <RotateCcw class="w-3.5 h-3.5 text-base-content/50" />
    </button>
  {/snippet}
</SettingsHeader>

<!-- Avatar -->
<div class="mb-6">
  <div class="text-xs font-semibold mb-1.5">{$t('settingsIdentity.avatar')}</div>
  <div class="flex items-center gap-4">
    <div class="w-16 h-16 rounded-xl bg-primary/15 text-primary grid place-items-center font-mono text-2xl font-semibold">{emoji || agentName?.charAt(0) || 'N'}</div>
    <div>
      <button class="px-3 py-1.5 rounded-lg border border-base-content/10 text-xs cursor-pointer hover:bg-base-200 transition-colors bg-transparent">{$t('settingsIdentity.uploadImage')}</button>
      <p class="text-xs text-base-content/50 mt-1">{$t('settingsIdentity.uploadLimits')}</p>
    </div>
  </div>
</div>

<!-- Agent name -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">{$t('settingsIdentity.employeeName')}</span>
  <input type="text" bind:value={agentName} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Emoji -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">{$t('settingsIdentity.emojiLabel')}</span>
  <input type="text" bind:value={emoji} oninput={debounceSave} class="w-20 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 text-center text-lg" />
</label>

<!-- Role -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">{$t('settingsIdentity.role')}</span>
  <input type="text" bind:value={role} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Persona section -->
<div class="mt-8 mb-7">
  <h3 class="text-sm font-semibold mb-3">{$t('settingsIdentity.persona')}</h3>
</div>

<!-- Creature -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">{$t('settingsIdentity.creatureArchetype')}</span>
  <select bind:value={creature} onchange={saveNow} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none cursor-pointer">
    <option value="">{$t('common.select')}</option>
    <option value="owl">{$t('settingsIdentity.creatureOwl')}</option>
    <option value="fox">{$t('settingsIdentity.creatureFox')}</option>
    <option value="bear">{$t('settingsIdentity.creatureBear')}</option>
    <option value="dolphin">{$t('settingsIdentity.creatureDolphin')}</option>
  </select>
</label>

<!-- Vibe -->
<label class="block mb-6">
  <span class="block text-xs font-semibold mb-1.5">{$t('settingsIdentity.vibe')}</span>
  <textarea rows="3" bind:value={vibe} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y" placeholder={$t('settingsIdentity.vibeDescPlaceholder')}></textarea>
</label>
