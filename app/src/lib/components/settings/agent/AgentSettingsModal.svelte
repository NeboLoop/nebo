<!--
  Employee settings, over the workspace. A modal rather than the side pane
  because these sections need room: persona and soul are full-height editors,
  channels embeds a chat, and configure runs the setup wizard. None of that
  belongs in a 450px rail — and none of it is being cut down.

  The nav is the ONE tab map in sections.ts: six everyday pages, and the
  Developer group below them in Developer mode. An old per-section name
  (?settings=persona) opens the page that section now lives on, at that
  section.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Building2 from 'lucide-svelte/icons/building-2';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import AgentSettingsView from './AgentSettingsView.svelte';
  import { visibleTabs, resolveSection, agentSettingsTabs } from './sections';
  import { botName, loadBotName } from '$lib/stores/botName';
  import { devMode } from '$lib/stores/devmode';

  let {
    open,
    agentId = '',
    section = 'general',
    agentName = '',
    agentRole = '',
    isPrimary = false,
    readOnly = false,
    avatarInitial = '',
    avatarClass = '',
    onsection,
    onclose
  }: {
    open: boolean;
    /** The employee, for the Developer group's doors to its other pages. */
    agentId?: string;
    /** The requested page: a tab id, or an old per-section name. */
    section?: string;
    agentName?: string;
    /** The employee's role, the title's second line. */
    agentRole?: string;
    /** The primary employee: the owner's personal assistant. */
    isPrimary?: boolean;
    readOnly?: boolean;
    /** The employee's roster identity chip — same avatar/color as the sidebar. */
    avatarInitial?: string;
    avatarClass?: string;
    onsection: (id: string) => void;
    onclose: () => void;
  } = $props();

  const resolved = $derived(resolveSection(section, $devMode));
  const tabs = $derived(visibleTabs($devMode));
  const everyday = $derived(tabs.filter((x) => !x.developer));
  const developer = $derived(tabs.filter((x) => x.developer));
  const currentLabel = $derived(agentSettingsTabs.find((x) => x.id === resolved.tab)?.label ?? 'nav.employeeSettings');

  // The title is the name only; the role goes on the second line, where it
  // may truncate without taking the name with it.
  const title = $derived(agentName || $t('nav.employeeSettings'));
  const subtitle = $derived(isPrimary ? $t('agentSettings.personalAssistant') : agentRole.trim());

  $effect(() => {
    if (open) void loadBotName();
  });

  // Phone: a 160px nav squeezed beside 200px of form is neither. Two steps
  // instead — the section list, then the section with a back chevron. Desktop
  // keeps both side by side. Opening deep-linked (?settings=persona) starts on
  // the content, which is what the link meant.
  let mobileDetail = $state(section !== 'general');

  function pickSection(id: string) {
    onsection(id);
    mobileDetail = true;
  }
</script>

{#snippet tabButton(id: string, label: string)}
  <button
    type="button"
    onclick={() => pickSection(id)}
    class="text-left py-1.5 max-md:py-3 px-2.5 max-md:px-3.5 rounded-md text-sm cursor-pointer transition-colors border flex items-center {resolved.tab === id
      ? 'bg-base-100 border-base-300 shadow-sm font-medium max-md:bg-transparent max-md:border-transparent max-md:shadow-none max-md:font-normal'
      : 'bg-transparent border-transparent hover:bg-base-200'}"
  >
    <span class="flex-1">{$t(label)}</span>
    <svg class="md:hidden w-3.5 h-3.5 text-base-content/30" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 3 11 8 6 13"/></svg>
  </button>
{/snippet}

<ShelfModal {open} {title} {subtitle} {avatarInitial} {avatarClass} {onclose}>
  {#snippet actions()}
    {#if readOnly}
      <span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs text-base-content/70" title={$t('agentSettings.identityManagedNote')}>{$t('agentSettings.readOnly')}</span>
    {/if}
  {/snippet}
  <div class="flex-1 min-h-0 flex flex-col">
    <!-- Where this employee works, and the way to the bot's own settings. -->
    <a href="/settings/account" class="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-base-300 text-xs text-base-content/70 hover:bg-base-200 no-underline">
      <Building2 class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
      <span class="truncate">{$botName ? $t('agentSettings.worksAt', { values: { bot: $botName } }) : $t('agentSettings.worksAtThisBot')}</span>
      <span class="ml-auto shrink-0 font-medium text-primary">{$t('nav.botSettings')} →</span>
    </a>
    <div class="flex-1 min-h-0 flex">
      <nav class="w-52 shrink-0 border-r border-base-300 bg-base-200/40 overflow-y-auto p-1.5 flex flex-col gap-0.5 max-md:w-full max-md:border-r-0 max-md:bg-transparent max-md:p-2.5 {mobileDetail ? 'max-md:hidden' : ''}">
        {#each everyday as sec (sec.id)}
          {@render tabButton(sec.id, sec.label)}
        {/each}
        {#if developer.length > 0}
          <div class="mt-3 mb-0.5 px-2.5 max-md:px-3.5 text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('agentSettings.developerGroup')}</div>
          {#each developer as sec (sec.id)}
            {#if sec.href}
              <a
                href={sec.href(agentId)}
                class="text-left py-1.5 max-md:py-3 px-2.5 max-md:px-3.5 rounded-md text-sm cursor-pointer transition-colors border border-transparent flex items-center no-underline text-base-content hover:bg-base-200"
              >
                <span class="flex-1">{$t(sec.label)}</span>
                <span class="text-base-content/40" aria-hidden="true">→</span>
              </a>
            {:else}
              {@render tabButton(sec.id, sec.label)}
            {/if}
          {/each}
        {/if}
      </nav>
      <div class="flex-1 min-w-0 min-h-0 flex flex-col {mobileDetail ? '' : 'max-md:hidden'}">
        <button
          type="button"
          class="md:hidden shrink-0 flex items-center gap-1.5 px-2.5 h-10 text-sm font-medium bg-transparent border-0 border-b border-base-300 cursor-pointer text-left"
          onclick={() => (mobileDetail = false)}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
          {$t(currentLabel)}
        </button>
        <AgentSettingsView tab={resolved.tab} focus={resolved.focus} />
      </div>
    </div>
  </div>
</ShelfModal>
