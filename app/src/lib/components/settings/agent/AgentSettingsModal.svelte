<!--
  Employee settings, over the workspace. A modal rather than the side pane
  because these sections need room: persona and soul are full-height editors,
  channels embeds a chat, and configure runs the setup wizard. None of that
  belongs in a 450px rail — and none of it is being cut down.

  The section list is the same one the old settings route used; the content is
  the same view, unmodified.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import AgentSettingsView from './AgentSettingsView.svelte';

  let {
    open,
    section = 'general',
    agentName = '',
    onsection,
    onclose
  }: {
    open: boolean;
    section?: string;
    agentName?: string;
    onsection: (id: string) => void;
    onclose: () => void;
  } = $props();

  // `label` holds an i18n key — translated with $t at render time.
  // Ambient capability permissions (file/shell/web…) are managed once,
  // globally, in Settings → Permissions and inherited by every employee.
  // Approvals is deliberately per-employee: the three-state control over each
  // employee's gated operations, so a global setting never overrides a
  // critical decision.
  const sections = [
    { id: 'general', label: 'agentSettings.general' },
    { id: 'identity', label: 'settings.navItems.identity' },
    { id: 'persona', label: 'agentPersona.title' },
    { id: 'soul', label: 'settings.navItems.soul' },
    { id: 'rules', label: 'settings.navItems.rules' },
    { id: 'configure', label: 'agent.configure' },
    { id: 'workflows', label: 'marketplace.workflows' },
    { id: 'skills', label: 'settings.navItems.skills' },
    { id: 'channels', label: 'agentSettings.channels' },
    { id: 'accounts', label: 'agentSettings.connectedAccounts' },
    { id: 'approvals', label: 'agentSettings.approvals' },
    { id: 'memory', label: 'agentSettings.memory' },
  ];

  const title = $derived(
    agentName ? `${agentName} — ${$t('settings.title')}` : $t('settings.title')
  );
</script>

<ShelfModal {open} {title} {onclose}>
  <nav class="w-52 shrink-0 border-r border-base-300 bg-base-200/40 overflow-y-auto p-1.5 flex flex-col gap-0.5 max-md:w-40">
    {#each sections as sec (sec.id)}
      <button
        type="button"
        onclick={() => onsection(sec.id)}
        class="text-left py-1.5 px-2.5 rounded-md text-sm cursor-pointer transition-colors border {section === sec.id
          ? 'bg-base-100 border-base-300 shadow-sm font-medium'
          : 'bg-transparent border-transparent hover:bg-base-200'}"
      >{$t(sec.label)}</button>
    {/each}
  </nav>
  <div class="flex-1 min-w-0 min-h-0 flex flex-col">
    <AgentSettingsView {section} />
  </div>
</ShelfModal>
