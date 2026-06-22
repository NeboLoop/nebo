<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import ApprovalModal from '$lib/components/ApprovalModal.svelte';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import Eye from 'lucide-svelte/icons/eye';
  import X from 'lucide-svelte/icons/x';

  // The capability list + labels come from the backend (tools::capabilities,
  // the single source of truth) via userGetPermissions().capabilities — NOT a
  // hardcoded list here, so the UI cannot drift from the gate's vocabulary.
  let permissions = $state<{ key: string; label: string; desc: string; enabled: boolean }[]>([]);
  let fullAccess = $state(false);
  // "Approve Always" shell-command prefixes — shown so the user can see + revoke them.
  let approvedCommands = $state<string[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [permResp, settingsResp] = await Promise.all([
        api.userGetPermissions(),
        api.getSettings().catch(() => null),
      ]);
      // Convert ToolPermission[] to a keyed record of current on/off values
      let permObj: Record<string, boolean> = {};
      if (permResp?.permissions?.length) {
        for (const tp of permResp.permissions) {
          permObj[tp.tool] = tp.allowed;
        }
      }
      // Render the toggle list from the backend's canonical capabilities
      // (key/label/desc). A capability with no persisted value defaults to on.
      permissions = (permResp?.capabilities ?? []).map(cap => ({
        key: cap.key,
        label: cap.label,
        desc: cap.desc,
        enabled: permObj[cap.key] ?? true,
      }));
      if (settingsResp?.settings?.fullAccess !== undefined) {
        fullAccess = !!settingsResp.settings.fullAccess;
      }
      approvedCommands = permResp?.approvedCommands ?? [];
    } catch { /* keep mock data */ }
  });

  async function removeApprovedCommand(pattern: string) {
    const next = approvedCommands.filter(p => p !== pattern);
    approvedCommands = next;
    try {
      const api = await import('$lib/api/nebo');
      await api.userUpdateApprovedCommands({ commands: next });
    } catch { /* keep local state */ }
  }
  let showPreview = $state(false);

  // Full Access activation modal state
  let showEnableModal = $state(false);
  let termsAccepted = $state(false);
  let confirmText = $state('');
  const canConfirm = $derived(termsAccepted && confirmText === 'ENABLE');

  function handleFullAccessToggle() {
    if (!fullAccess) {
      // Turning ON — show confirmation modal
      showEnableModal = true;
    } else {
      // Turning OFF — just disable
      fullAccess = false;
      saveFullAccess(false);
    }
  }

  function cancelEnable() {
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
  }

  async function confirmEnable() {
    if (!canConfirm) return;
    fullAccess = true;
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
    await saveFullAccess(true);
  }

  async function saveFullAccess(enabled: boolean) {
    try {
      const api = await import('$lib/api/nebo');
      await api.updateSettings({ fullAccess: enabled });
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

<!-- Full Access -->
<div class="flex items-center justify-between p-4 rounded-xl border border-base-300 mb-7">
  <div>
    <div class="text-sm font-semibold flex items-center gap-2">
      {#if fullAccess}<AlertTriangle class="w-4 h-4 text-warning" />{/if}
      Full Access
    </div>
    <div class="text-xs text-base-content/70">The agent will execute all tools without asking for permission.</div>
  </div>
  <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={fullAccess} onchange={handleFullAccessToggle} />
</div>

{#if fullAccess}
  <div class="rounded-xl bg-warning/10 border border-warning/20 px-4 py-3 mb-7">
    <p class="text-xs text-warning font-medium">Full Access is active</p>
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
        checked={fullAccess || perm.enabled}
        disabled={fullAccess}
        onchange={() => toggleCapability(perm.key)}
      />
    </div>
  {/each}
</div>

<!-- Always-approved commands (per-command allowlist) -->
{#if approvedCommands.length > 0}
  <h3 class="text-sm font-semibold mb-1">Always-approved commands</h3>
  <p class="text-xs text-base-content/70 mb-3">Shell command prefixes you chose "Approve Always" for — they run without asking. Dangerous commands are still blocked.</p>
  <div class="divide-y divide-base-content/10 mb-7">
    {#each approvedCommands as cmd}
      <div class="flex items-center justify-between py-2.5">
        <code class="text-sm font-mono">{cmd}</code>
        <button
          onclick={() => removeApprovedCommand(cmd)}
          class="w-7 h-7 flex items-center justify-center rounded-lg hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent text-base-content/60 hover:text-error"
          aria-label="Remove {cmd}"
        >
          <X class="w-4 h-4" />
        </button>
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

<!-- Full Access Activation Modal -->
{#if showEnableModal}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="button" tabindex="0" aria-label="Close modal" onclick={cancelEnable} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); cancelEnable(); } }}></div>
    <div class="relative w-full max-w-lg rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-sm font-bold">Enable Full Access</h3>
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
            By enabling Full Access, you acknowledge that Nebo Labs, Inc. shall not be liable for any damages, losses, or consequences arising from the unsupervised execution of tools by the agent. You accept full responsibility for all actions taken by the agent while Full Access is enabled.
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
          Enable Full Access
        </button>
      </div>
    </div>
  </div>
{/if}
