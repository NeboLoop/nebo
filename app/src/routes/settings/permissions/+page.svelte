<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import ApprovalModal from '$lib/components/ApprovalModal.svelte';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import Eye from 'lucide-svelte/icons/eye';
  import X from 'lucide-svelte/icons/x';

  const CAPABILITY_LABELS: Record<string, { label: string; desc: string }> = {
    chat: { label: 'Chat', desc: 'Respond to messages and conversations' },
    file: { label: 'File Access', desc: 'Read and write files on your system' },
    shell: { label: 'Shell Commands', desc: 'Execute terminal commands' },
    web: { label: 'Web Access', desc: 'Make HTTP requests and browse the web' },
    contacts: { label: 'Contacts', desc: 'Access your contacts and address book' },
    desktop: { label: 'Desktop', desc: 'Control mouse, keyboard, and windows' },
    media: { label: 'Media', desc: 'Access camera, microphone, and screen' },
    system: { label: 'System', desc: 'Access system information and settings' },
  };

  let permissions = $state<{ key: string; label: string; desc: string; enabled: boolean }[]>([]);
  let autonomous = $state(false);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [permResp, settingsResp] = await Promise.all([
        api.userGetPermissions(),
        api.getSettings().catch(() => null),
      ]);
      // Convert ToolPermission[] to a keyed record
      let permObj: Record<string, boolean> = {};
      if (permResp?.permissions?.length) {
        for (const tp of permResp.permissions) {
          permObj[tp.tool] = tp.allowed;
        }
      }
      // Build capability list from known keys + any extras from backend
      const allKeys = new Set([...Object.keys(CAPABILITY_LABELS), ...Object.keys(permObj)]);
      permissions = Array.from(allKeys).map(key => ({
        key,
        label: CAPABILITY_LABELS[key]?.label || key.charAt(0).toUpperCase() + key.slice(1),
        desc: CAPABILITY_LABELS[key]?.desc || '',
        enabled: permObj[key] ?? true,
      }));
      if (settingsResp?.settings?.autonomousMode !== undefined) {
        autonomous = !!settingsResp.settings.autonomousMode;
      }
    } catch { /* keep mock data */ }
  });
  let showPreview = $state(false);

  // Autonomous activation modal state
  let showEnableModal = $state(false);
  let termsAccepted = $state(false);
  let confirmText = $state('');
  const canConfirm = $derived(termsAccepted && confirmText === 'ENABLE');

  function handleAutonomousToggle() {
    if (!autonomous) {
      // Turning ON — show confirmation modal
      showEnableModal = true;
    } else {
      // Turning OFF — just disable
      autonomous = false;
      saveAutonomousMode(false);
    }
  }

  function cancelEnable() {
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
  }

  async function confirmEnable() {
    if (!canConfirm) return;
    autonomous = true;
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
    await saveAutonomousMode(true);
  }

  async function saveAutonomousMode(enabled: boolean) {
    try {
      const api = await import('$lib/api/nebo');
      await api.updateSettings({ autonomousMode: enabled });
    } catch { /* keep local state */ }
  }

  async function toggleCapability(key: string) {
    const perm = permissions.find(p => p.key === key);
    if (!perm) return;
    perm.enabled = !perm.enabled;
    try {
      const api = await import('$lib/api/nebo');
      const permObj: Record<string, boolean> = {};
      for (const p of permissions) {
        permObj[p.key] = p.enabled;
      }
      await api.userUpdatePermissions({ permissions: permObj });
    } catch { /* keep local state */ }
  }
</script>

<SettingsHeader title="Permissions" description="Control what your agent can access and do." />

<!-- Autonomous mode -->
<div class="flex items-center justify-between p-4 rounded-xl border border-base-300 mb-7">
  <div>
    <div class="text-sm font-semibold flex items-center gap-2">
      {#if autonomous}<AlertTriangle class="w-4 h-4 text-warning" />{/if}
      Autonomous Mode
    </div>
    <div class="text-xs text-base-content/70">The agent will execute all tools without asking for permission.</div>
  </div>
  <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={autonomous} onchange={handleAutonomousToggle} />
</div>

{#if autonomous}
  <div class="rounded-xl bg-warning/10 border border-warning/20 px-4 py-3 mb-7">
    <p class="text-xs text-warning font-medium">Autonomous Mode is active</p>
    <p class="text-xs text-base-content/70 mt-0.5">All approval prompts are bypassed. Make sure you trust the prompts you're sending.</p>
  </div>
{/if}

<!-- Capabilities -->
<h3 class="text-sm font-semibold mb-3">Capabilities</h3>
<div class="divide-y divide-base-content/10 mb-7">
  {#each permissions as perm}
    <div class="flex items-center justify-between py-3.5">
      <div>
        <div class="text-sm font-medium">{perm.label}</div>
        <div class="text-xs text-base-content/70">{perm.desc}</div>
      </div>
      <input
        type="checkbox"
        class="toggle toggle-sm toggle-primary"
        checked={autonomous || perm.enabled}
        disabled={autonomous}
        onchange={() => toggleCapability(perm.key)}
      />
    </div>
  {/each}
</div>

<!-- Tool auto-approval -->
{#if !autonomous}
  <h3 class="text-sm font-semibold mb-1">Auto-approval</h3>
  <p class="text-xs text-base-content/70 mb-3">Actions the agent can take without asking you first.</p>
  <div class="divide-y divide-base-content/10 mb-7">
    {#each ['File reads', 'File writes', 'Bash commands', 'Web requests'] as tool}
      <div class="flex items-center justify-between py-3.5">
        <span class="text-sm font-medium">{tool}</span>
        <input type="checkbox" class="toggle toggle-sm toggle-primary" />
      </div>
    {/each}
  </div>
{/if}

<!-- Approval dialog preview -->
<h3 class="text-sm font-semibold mb-1">Approval Dialog</h3>
<p class="text-xs text-base-content/70 mb-3">When an agent needs permission, this dialog appears.</p>
<button
  onclick={() => showPreview = true}
  class="inline-flex items-center gap-2 px-4 py-2 rounded-lg border border-base-300 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
>
  <Eye class="w-4 h-4" /> Preview Approval Dialog
</button>

<ApprovalModal
  bind:show={showPreview}
  agent="Research Agent"
  actionType="shell_command"
  actionDetail="curl -s https://api.example.com/data | jq '.results[]'"
  actionKey="preview"
/>

<!-- Autonomous Mode Activation Modal -->
{#if showEnableModal}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="button" tabindex="0" aria-label="Close modal" onclick={cancelEnable} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); cancelEnable(); } }}></div>
    <div class="relative w-full max-w-lg rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-sm font-bold">Enable Autonomous Mode</h3>
        <button onclick={cancelEnable} class="w-7 h-7 flex items-center justify-center rounded-lg hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent">
          <X class="w-4 h-4" />
        </button>
      </div>

      <div class="px-5 py-4 space-y-4">
        <!-- Warning -->
        <div class="flex items-start gap-3">
          <AlertTriangle class="w-5 h-5 text-warning shrink-0 mt-0.5" />
          <p class="text-xs text-base-content leading-relaxed">
            This will allow your agent to execute all tools — including shell commands, file modifications, and network requests — without asking for permission.
          </p>
        </div>

        <!-- Risks -->
        <div class="rounded-xl bg-error/10 border border-error/20 p-4">
          <p class="text-xs font-semibold text-error mb-2">Risks include:</p>
          <ul class="text-xs text-base-content/70 space-y-1 list-disc list-inside">
            <li>The agent may modify or delete files on your system</li>
            <li>The agent may execute arbitrary shell commands</li>
            <li>The agent may make network requests and access external services</li>
            <li>You are solely responsible for any actions taken by the agent</li>
          </ul>
        </div>

        <!-- Disclaimer -->
        <div class="rounded-xl bg-base-200 p-4 max-h-28 overflow-y-auto">
          <p class="text-xs text-base-content/70 leading-relaxed">
            By enabling autonomous mode, you acknowledge that Nebo Labs, Inc. shall not be liable for any damages, losses, or consequences arising from the autonomous execution of tools by the agent. You accept full responsibility for all actions taken by the agent while autonomous mode is enabled.
          </p>
        </div>

        <!-- Accept checkbox -->
        <label class="flex items-center gap-3 cursor-pointer">
          <input type="checkbox" class="checkbox checkbox-sm checkbox-warning" bind:checked={termsAccepted} />
          <span class="text-xs font-medium">I understand the risks and accept full responsibility</span>
        </label>

        <!-- Type ENABLE -->
        <div>
          <label class="block text-xs font-medium mb-1.5" for="confirm-enable">Type ENABLE to confirm</label>
          <input
            id="confirm-enable"
            type="text"
            class="w-full h-9 rounded-lg bg-base-200 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
            placeholder="ENABLE"
            bind:value={confirmText}
            onkeydown={(e) => { if (e.key === 'Enter' && canConfirm) confirmEnable(); }}
          />
        </div>
      </div>

      <!-- Footer -->
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button
          onclick={cancelEnable}
          class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
        >
          Cancel
        </button>
        <button
          onclick={confirmEnable}
          disabled={!canConfirm}
          class="px-4 py-2 rounded-lg bg-error text-error-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-30 disabled:cursor-not-allowed"
        >
          Enable Autonomous Mode
        </button>
      </div>
    </div>
  </div>
{/if}
