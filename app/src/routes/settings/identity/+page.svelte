<script lang="ts">
  import { onMount } from 'svelte';
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

<div class="mb-7">
  <div class="flex items-center justify-between">
    <div>
      <h2 class="text-lg font-bold mb-1">Identity</h2>
      <p class="text-xs text-base-content/70">Configure your agent's name, avatar, and persona.</p>
    </div>
    <div class="flex items-center gap-2">
      {#if saved}
        <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
      {/if}
      <button onclick={revert} class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none" title="Revert to last saved">
        <RotateCcw class="w-3.5 h-3.5 text-base-content/50" />
      </button>
    </div>
  </div>
</div>

<!-- Avatar -->
<div class="mb-6">
  <div class="text-xs font-semibold mb-1.5">Avatar</div>
  <div class="flex items-center gap-4">
    <div class="w-16 h-16 rounded-xl bg-primary/15 text-primary grid place-items-center font-mono text-2xl font-semibold">{emoji || agentName?.charAt(0) || 'N'}</div>
    <div>
      <button class="px-3 py-1.5 rounded-lg border border-base-content/10 text-xs cursor-pointer hover:bg-base-200 transition-colors bg-transparent">Upload Image</button>
      <p class="text-xs text-base-content/50 mt-1">Max 512KB. PNG or JPG.</p>
    </div>
  </div>
</div>

<!-- Agent name -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Agent Name</span>
  <input type="text" bind:value={agentName} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Emoji -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Emoji</span>
  <input type="text" bind:value={emoji} oninput={debounceSave} class="w-20 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 text-center text-lg" />
</label>

<!-- Role -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Role</span>
  <input type="text" bind:value={role} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Persona section -->
<div class="mt-8 mb-7">
  <h3 class="text-sm font-semibold mb-3">Persona</h3>
</div>

<!-- Creature -->
<label class="block mb-4">
  <span class="block text-xs font-semibold mb-1.5">Creature Archetype</span>
  <select bind:value={creature} onchange={saveNow} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none cursor-pointer">
    <option value="">Select...</option>
    <option value="owl">Owl (Wise & observant)</option>
    <option value="fox">Fox (Clever & adaptable)</option>
    <option value="bear">Bear (Strong & reliable)</option>
    <option value="dolphin">Dolphin (Smart & playful)</option>
  </select>
</label>

<!-- Vibe -->
<label class="block mb-6">
  <span class="block text-xs font-semibold mb-1.5">Vibe</span>
  <textarea rows="3" bind:value={vibe} oninput={debounceSave} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y" placeholder="Describe the personality vibe..."></textarea>
</label>
