<script lang="ts">
  import { onMount } from 'svelte';
  import { theme } from '$lib/stores/theme.js';

  let user = $state({ displayName: '', occupation: '', location: '', timezone: '', interests: [] as string[], goals: '', commStyle: 'adaptive' });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getAgentProfile() as { profile?: Record<string, unknown> } | null;
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
      }
    } catch { /* keep mock data */ }
  });

  async function saveChanges() {
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgentProfile({
        displayName: user.displayName,
        occupation: user.occupation,
        location: user.location,
        timezone: user.timezone,
        interests: user.interests,
        goals: user.goals,
        communicationStyle: user.commStyle,
      });
    } catch { /* ignore */ }
  }

  const themes = [
    { id: 'nebo', name: 'Nebo', colors: { bg: '#fcfdfe', surface: '#f3f7f8', primary: '#006d7f', text: '#17202a', muted: '#d8e1e6' } },
    { id: 'light', name: 'Light', colors: { bg: '#ffffff', surface: '#ffffff', primary: '#570df8', text: '#1f2937', muted: '#e5e7eb' } },
    { id: 'dark', name: 'Dark', colors: { bg: '#1d232a', surface: '#2a323c', primary: '#661ae6', text: '#a6adba', muted: '#373f4a' } },
    { id: 'cupcake', name: 'Cupcake', colors: { bg: '#faf7f5', surface: '#ffffff', primary: '#65c3c8', text: '#291334', muted: '#e8e0d8' } },
    { id: 'nord', name: 'Nord', colors: { bg: '#eceff4', surface: '#e5e9f0', primary: '#5e81ac', text: '#2e3440', muted: '#d8dee9' } },
    { id: 'sunset', name: 'Sunset', colors: { bg: '#1a1028', surface: '#241835', primary: '#ff865b', text: '#f5e6d3', muted: '#352340' } },
    { id: 'autumn', name: 'Autumn', colors: { bg: '#f1f1f1', surface: '#ffffff', primary: '#8c0327', text: '#1b1b1b', muted: '#d8d5cf' } },
    { id: 'lemonade', name: 'Lemonade', colors: { bg: '#f5fbeb', surface: '#ffffff', primary: '#519903', text: '#1b2d00', muted: '#dce9c5' } },
    { id: 'night', name: 'Night', colors: { bg: '#0f1729', surface: '#1a2332', primary: '#38bdf8', text: '#b6cedb', muted: '#253445' } },
    { id: 'coffee', name: 'Coffee', colors: { bg: '#20161f', surface: '#2a1f29', primary: '#db924b', text: '#c8b6a6', muted: '#362c35' } },
    { id: 'winter', name: 'Winter', colors: { bg: '#f0f4f8', surface: '#ffffff', primary: '#047aff', text: '#394e6a', muted: '#d4dce8' } },
  ];
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Profile</h2>
  <p class="text-xs text-base-content/70">Your personal information and preferences.</p>
</div>

<!-- Theme -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-2">Theme</div>
  <div class="grid grid-cols-5 gap-2">
    {#each themes as t}
      <button
        class="rounded-lg border-2 overflow-hidden cursor-pointer transition-all {$theme === t.id ? 'border-primary shadow-sm' : 'border-base-content/10 hover:border-base-content/25'}"
        onclick={() => $theme = t.id}
      >
        <!-- Mini preview -->
        <div class="h-[52px] p-1.5 flex flex-col gap-1" style="background:{t.colors.bg}">
          <div class="flex items-center gap-1">
            <div class="w-2 h-2 rounded-sm shrink-0" style="background:{t.colors.primary}"></div>
            <div class="h-1.5 flex-1 rounded-sm" style="background:{t.colors.muted}"></div>
          </div>
          <div class="flex gap-1 flex-1">
            <div class="w-[30%] rounded-sm" style="background:{t.colors.muted}"></div>
            <div class="flex-1 rounded-sm" style="background:{t.colors.surface}"></div>
          </div>
        </div>
        <div class="py-1.5 px-2 text-sm font-medium text-center" style="background:{t.colors.bg}; color:{t.colors.text}">{t.name}</div>
      </button>
    {/each}
  </div>
</div>

<!-- Display name -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Display Name</span>
  <input type="text" bind:value={user.displayName} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Occupation -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Occupation</span>
  <input type="text" bind:value={user.occupation} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Location -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Location</span>
  <input type="text" bind:value={user.location} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Timezone -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Timezone</span>
  <div class="flex gap-2">
    <select class="flex-1 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none cursor-pointer">
      <option>{user.timezone}</option>
    </select>
    <button class="px-3 py-2 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">Auto-detect</button>
  </div>
</label>

<!-- Interests -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Interests</span>
  <div class="flex flex-wrap gap-1.5 mb-2">
    {#each user.interests as interest}
      <span class="inline-flex items-center gap-1 px-2 py-1 rounded bg-base-200 text-sm">
        {interest} <button class="text-base-content hover:text-base-content cursor-pointer">×</button>
      </span>
    {/each}
  </div>
  <input type="text" placeholder="Add interest…" class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 placeholder:text-base-content" />
</label>

<!-- Goals -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Goals</span>
  <textarea rows="2" class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y" bind:value={user.goals}></textarea>
</label>

<!-- Communication style -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-1.5">Communication Style</div>
  <div class="flex gap-1.5">
    {#each ['casual', 'professional', 'adaptive'] as style}
      <button class="px-3.5 py-1.5 rounded-lg border text-sm cursor-pointer transition-colors {user.commStyle === style
        ? 'bg-primary/10 text-primary border-primary font-medium'
        : 'border-base-content/10 bg-base-100 hover:bg-base-200'}"
        onclick={() => user.commStyle = style}>{style.charAt(0).toUpperCase() + style.slice(1)}</button>
    {/each}
  </div>
</div>

<button class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer hover:opacity-90 transition-opacity" onclick={saveChanges}>Save Changes</button>
