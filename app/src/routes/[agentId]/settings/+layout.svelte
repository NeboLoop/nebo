<script lang="ts">
  import { page } from '$app/stores';
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import { mobileChatsOpen } from '$lib/stores/mobileNav';
  import type { AgentPageContext } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));

  // `label` holds an i18n key — translated with $t at render time.
  const settingsSections = [
    { id: 'general', label: 'agentSettings.general' },
    { id: 'identity', label: 'settings.navItems.identity' },
    { id: 'persona', label: 'agentPersona.title' },
    { id: 'soul', label: 'settings.navItems.soul' },
    { id: 'configure', label: 'agent.configure' },
    { id: 'workflows', label: 'marketplace.workflows' },
    { id: 'skills', label: 'settings.navItems.skills' },
    { id: 'channels', label: 'agentSettings.channels' },
    { id: 'accounts', label: 'agentSettings.connectedAccounts' },
    { id: 'memory', label: 'agentSettings.memory' },
    // Permissions are managed once, globally, in Settings → Permissions and
    // inherited by every agent. There is no per-agent permissions surface —
    // a single source of truth is the right model for non-technical users.
  ];

  const activeSection = $derived($page.params.section || 'general');
</script>

<!-- Column 2: Settings nav (mobile: slide-over toggled from the settings bar) -->
{#if $mobileChatsOpen}
  <div class="fixed inset-0 z-30 bg-black/40 md:hidden" onclick={() => mobileChatsOpen.set(false)} role="presentation"></div>
{/if}
<div class="md:w-[260px] md:min-w-[260px] max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:w-[280px] max-md:transition-transform {$mobileChatsOpen ? 'max-md:translate-x-0 max-md:shadow-2xl' : 'max-md:-translate-x-full'} border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50 max-md:bg-base-200">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} isApp={ctx.isApp} />

  <div class="flex-1 overflow-y-auto">
    <div class="p-1.5 flex flex-col gap-0.5">
      {#each settingsSections as sec}
        <a
          href="/{agentId}/settings/{sec.id}"
          class="flex items-center w-full text-left py-1.5 px-2.5 rounded-md text-sm cursor-pointer transition-colors no-underline text-base-content {activeSection === sec.id ? 'bg-base-100 border border-base-300 shadow-sm font-medium' : 'bg-transparent border border-transparent hover:bg-base-200'}"
        >{$t(sec.label)}</a>
      {/each}
    </div>
  </div>
</div>

<!-- Column 3: Settings detail from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  <!-- Mobile settings bar: the drawer toggle (section nav is a slide-over below md) -->
  <div class="md:hidden h-10 shrink-0 border-b border-base-300 flex items-center gap-2 px-2">
    <button
      class="h-8 px-2.5 rounded-md flex items-center gap-1.5 text-sm font-medium border-none bg-transparent cursor-pointer text-base-content/80"
      onclick={() => mobileChatsOpen.update((v) => !v)}
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>
      {$t('settings.title')}
    </button>
    <span class="text-sm text-base-content/60 truncate">{$t(settingsSections.find((s) => s.id === activeSection)?.label ?? 'settings.title')}</span>
  </div>
  {@render children()}
</div>
