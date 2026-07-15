<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { themeMode, uiScale, type UiScale } from '$lib/stores/theme.js';
  import Check from 'lucide-svelte/icons/check';
  import RotateCcw from 'lucide-svelte/icons/rotate-ccw';
  import Sun from 'lucide-svelte/icons/sun';
  import Moon from 'lucide-svelte/icons/moon';
  import Monitor from 'lucide-svelte/icons/monitor';

  let user = $state({ displayName: '', occupation: '', location: '', timezone: '', interests: [] as string[], goals: '', commStyle: 'adaptive' });
  let saved = $state(false);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let newInterest = $state('');

  // Snapshot for revert
  let snapshot = $state({ displayName: '', occupation: '', location: '', timezone: '', interests: [] as string[], goals: '', commStyle: 'adaptive' });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.userGetProfile() as unknown as { profile?: Record<string, unknown> };
      if (resp?.profile) {
        const p = resp.profile;
        user = {
          ...user,
          displayName: String(p.displayName || user.displayName),
          occupation: String(p.occupation || user.occupation),
          location: String(p.location || user.location),
          timezone: String(p.timezone || user.timezone),
          interests: (p.interests as string[]) || user.interests,
          goals: String(p.goals || user.goals),
          commStyle: String(p.communicationStyle || user.commStyle),
        };
        snapshot = { ...user, interests: [...user.interests] };
      }
    } catch { /* keep defaults */ }
  });

  function debounceSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => persistProfile(), 800);
  }

  async function persistProfile() {
    try {
      const api = await import('$lib/api/nebo');
      await api.userUpdateProfile({
        displayName: user.displayName,
        occupation: user.occupation,
        location: user.location,
        timezone: user.timezone,
        interests: user.interests,
        goals: user.goals,
        communicationStyle: user.commStyle,
      });
      snapshot = { ...user, interests: [...user.interests] };
      saved = true;
      setTimeout(() => saved = false, 2000);
    } catch { /* silent */ }
  }

  function revert() {
    user = { ...snapshot, interests: [...snapshot.interests] };
  }

  function setCommStyle(style: string) {
    user.commStyle = style;
    persistProfile();
  }

  function removeInterest(idx: number) {
    user.interests = user.interests.filter((_, i) => i !== idx);
    persistProfile();
  }

  function addInterest() {
    const val = newInterest.trim();
    if (!val || user.interests.includes(val)) return;
    user.interests = [...user.interests, val];
    newInterest = '';
    persistProfile();
  }

  function autoDetectTimezone() {
    user.timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    persistProfile();
  }

  const themeOptions = [
    { id: 'light' as const, label: 'Light', icon: Sun },
    { id: 'dark' as const, label: 'Dark', icon: Moon },
    { id: 'system' as const, label: 'System', icon: Monitor },
  ];

  // Whole-UI zoom steps. The "A" glyph size hints the scale.
  const scaleOptions: { v: UiScale; label: string; glyph: string }[] = [
    { v: 0.9, label: 'Small', glyph: 'text-xs' },
    { v: 1, label: 'Default', glyph: 'text-sm' },
    { v: 1.1, label: 'Large', glyph: 'text-base' },
    { v: 1.25, label: 'Extra Large', glyph: 'text-lg' },
  ];
</script>

<SettingsHeader title="Profile" description="Your personal information and preferences.">
  {#snippet action()}
    {#if saved}
      <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
    {/if}
    <button onclick={revert} class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none" title="Revert to last saved">
      <RotateCcw class="w-3.5 h-3.5 text-base-content/50" />
    </button>
  {/snippet}
</SettingsHeader>

<!-- Theme -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-2">Theme</div>
  <div class="grid grid-cols-3 gap-2 max-w-md">
    {#each themeOptions as opt}
      {@const Icon = opt.icon}
      <button
        class="flex flex-col items-center gap-2 py-4 px-3 rounded-lg border transition-colors cursor-pointer {$themeMode === opt.id ? 'border-primary bg-primary/5' : 'border-base-content/10 hover:border-base-content/25 bg-transparent'}"
        onclick={() => themeMode.set(opt.id)}
      >
        <Icon class="w-5 h-5 {$themeMode === opt.id ? 'text-primary' : 'text-base-content/70'}" />
        <span class="text-xs font-medium {$themeMode === opt.id ? 'text-primary' : 'text-base-content'}">{opt.label}</span>
      </button>
    {/each}
  </div>
</div>

<!-- Font size -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-2">Font size</div>
  <div class="grid grid-cols-4 gap-2 max-w-md">
    {#each scaleOptions as opt}
      <button
        class="flex flex-col items-center gap-2 py-4 px-3 rounded-lg border transition-colors cursor-pointer {$uiScale === opt.v ? 'border-primary bg-primary/5' : 'border-base-content/10 hover:border-base-content/25 bg-transparent'}"
        onclick={() => uiScale.set(opt.v)}
      >
        <span class="{opt.glyph} font-semibold leading-none {$uiScale === opt.v ? 'text-primary' : 'text-base-content/70'}">A</span>
        <span class="text-xs font-medium {$uiScale === opt.v ? 'text-primary' : 'text-base-content'}">{opt.label}</span>
      </button>
    {/each}
  </div>
</div>

<!-- Display name -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Display Name</span>
  <input type="text" bind:value={user.displayName} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Occupation -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Occupation</span>
  <input type="text" bind:value={user.occupation} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Location -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Location</span>
  <input type="text" bind:value={user.location} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Timezone -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Timezone</span>
  <div class="flex gap-2">
    <input type="text" bind:value={user.timezone} oninput={debounceSave} class="flex-1 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
    <button class="px-3 py-2 rounded-lg border border-base-content/10 text-xs cursor-pointer hover:bg-base-200 transition-colors bg-transparent" onclick={autoDetectTimezone}>Auto-detect</button>
  </div>
</label>

<!-- Interests -->
<div class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Interests</span>
  <div class="flex flex-wrap gap-1.5 mb-2">
    {#each user.interests as interest, idx}
      <span class="inline-flex items-center gap-1 px-2 py-1 rounded bg-base-200 text-xs">
        {interest} <button class="text-base-content hover:text-error cursor-pointer bg-transparent border-none p-0 text-xs" onclick={() => removeInterest(idx)}>×</button>
      </span>
    {/each}
  </div>
  <input type="text" bind:value={newInterest} placeholder="Add interest…" class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30"
    onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addInterest(); } }} />
</div>

<!-- Goals -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Goals</span>
  <textarea rows="2" class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y" bind:value={user.goals} oninput={debounceSave}></textarea>
</label>

<!-- Communication style -->
<div class="mb-6">
  <div class="text-xs font-semibold mb-1.5">Communication Style</div>
  <div class="flex gap-1.5">
    {#each ['casual', 'professional', 'adaptive'] as style}
      <button class="px-3.5 py-1.5 rounded-lg border text-xs cursor-pointer transition-colors {user.commStyle === style
        ? 'bg-primary/10 text-primary border-primary font-medium'
        : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
        onclick={() => setCommStyle(style)}>{style.charAt(0).toUpperCase() + style.slice(1)}</button>
    {/each}
  </div>
</div>
