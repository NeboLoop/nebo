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
  import { agentSettingsSections } from './sections';

  let {
    open,
    section = 'general',
    agentName = '',
    readOnly = false,
    avatarInitial = '',
    avatarClass = '',
    onsection,
    onclose
  }: {
    open: boolean;
    section?: string;
    agentName?: string;
    readOnly?: boolean;
    /** The employee's roster identity chip — same avatar/color as the sidebar. */
    avatarInitial?: string;
    avatarClass?: string;
    onsection: (id: string) => void;
    onclose: () => void;
  } = $props();

  const sections = agentSettingsSections;

  const title = $derived(
    agentName ? `${agentName} — ${$t('settings.title')}` : $t('settings.title')
  );

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

<ShelfModal {open} {title} {avatarInitial} {avatarClass} {onclose}>
  {#snippet actions()}
    {#if readOnly}
      <span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs text-base-content/70" title={$t('agentSettings.identityManagedNote')}>{$t('agentSettings.readOnly')}</span>
    {/if}
  {/snippet}
  <nav class="w-52 shrink-0 border-r border-base-300 bg-base-200/40 overflow-y-auto p-1.5 flex flex-col gap-0.5 max-md:w-full max-md:border-r-0 max-md:bg-transparent max-md:p-2.5 {mobileDetail ? 'max-md:hidden' : ''}">
    {#each sections as sec (sec.id)}
      <button
        type="button"
        onclick={() => pickSection(sec.id)}
        class="text-left py-1.5 max-md:py-3 px-2.5 max-md:px-3.5 rounded-md text-sm cursor-pointer transition-colors border flex items-center {section === sec.id
          ? 'bg-base-100 border-base-300 shadow-sm font-medium max-md:bg-transparent max-md:border-transparent max-md:shadow-none max-md:font-normal'
          : 'bg-transparent border-transparent hover:bg-base-200'}"
      >
        <span class="flex-1">{$t(sec.label)}</span>
        <svg class="md:hidden w-3.5 h-3.5 text-base-content/30" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 3 11 8 6 13"/></svg>
      </button>
    {/each}
  </nav>
  <div class="flex-1 min-w-0 min-h-0 flex flex-col {mobileDetail ? '' : 'max-md:hidden'}">
    <button
      type="button"
      class="md:hidden shrink-0 flex items-center gap-1.5 px-2.5 h-10 text-sm font-medium bg-transparent border-0 border-b border-base-300 cursor-pointer text-left"
      onclick={() => (mobileDetail = false)}
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
      {$t(sections.find((x) => x.id === section)?.label ?? 'settings.title')}
    </button>
    <AgentSettingsView {section} />
  </div>
</ShelfModal>
