<script lang="ts">
  import { onMount } from 'svelte';

  let agentName = $state('');
  let emoji = $state('');
  let role = $state('');
  let creature = $state('');
  let vibe = $state('');
  let saving = $state(false);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getProfile() as { profile?: Record<string, unknown> } | null;
      if (resp?.profile) {
        const p = resp.profile;
        agentName = String(p.name || '');
        emoji = String(p.emoji || '');
        role = String(p.role || '');
        creature = String(p.creature || '');
        vibe = String(p.vibe || '');
      }
    } catch { /* keep defaults */ }
  });

  async function save() {
    saving = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateProfile({
        name: agentName || undefined,
        emoji: emoji || undefined,
        role: role || undefined,
        creature: creature || undefined,
        vibe: vibe || undefined,
      });
    } catch { /* silent */ }
    saving = false;
  }
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Identity</h2>
  <p class="text-xs text-base-content/70">Configure your agent's name, avatar, and persona.</p>
</div>

<!-- Avatar -->
<div class="mb-6">
  <div class="text-sm font-semibold mb-1.5">Avatar</div>
  <div class="flex items-center gap-4">
    <div class="w-16 h-16 rounded-xl bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)] grid place-items-center font-mono text-2xl font-semibold">{emoji || agentName?.charAt(0) || 'N'}</div>
    <div>
      <button class="px-3 py-1.5 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">Upload Image</button>
      <p class="text-sm mt-1">Max 512KB. PNG or JPG.</p>
    </div>
  </div>
</div>

<!-- Agent name -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Agent Name</span>
  <input type="text" bind:value={agentName} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Emoji -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Emoji</span>
  <input type="text" bind:value={emoji} class="w-20 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 text-center text-lg" />
</label>

<!-- Role -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Role</span>
  <input type="text" bind:value={role} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30" />
</label>

<!-- Persona section -->
<div class="mt-8 mb-7">
  <h3 class="text-base font-semibold mb-3">Persona</h3>
</div>

<!-- Creature -->
<label class="block mb-4">
  <span class="block text-sm font-semibold mb-1.5">Creature Archetype</span>
  <select bind:value={creature} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none cursor-pointer">
    <option value="">Select...</option>
    <option value="owl">Owl (Wise & observant)</option>
    <option value="fox">Fox (Clever & adaptable)</option>
    <option value="bear">Bear (Strong & reliable)</option>
    <option value="dolphin">Dolphin (Smart & playful)</option>
  </select>
</label>

<!-- Vibe -->
<label class="block mb-6">
  <span class="block text-sm font-semibold mb-1.5">Vibe</span>
  <textarea rows="3" bind:value={vibe} class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-y" placeholder="Describe the personality vibe..."></textarea>
</label>

<button onclick={save} disabled={saving} class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer hover:opacity-90 transition-opacity disabled:opacity-50">{saving ? 'Saving...' : 'Save Changes'}</button>
