<script lang="ts">
  import { t } from 'svelte-i18n';
  import { parseChangeText, countChanges, type ChangeKind } from '$lib/layers/diff';

  // The change text, rendered the way a pull request renders one: a heading per
  // file, hunks kept apart, removed lines red and added lines green, monospace,
  // and the long lines scrolling inside their own block so the page never does.
  //
  // This is the same text the employees read in their update run, so what is on
  // this screen is what they get.
  //
  // `kind` is what the pending entry says happened to the pack as a whole. A
  // pack that has just arrived carries its whole text and no per-file markers,
  // so without it a brand-new layer would read as if nothing were new.
  let { text, kind }: { text: string; kind?: ChangeKind } = $props();

  const set = $derived(parseChangeText(text, kind));
  const counts = $derived(countChanges(set));

  const KIND_BADGE: Record<ChangeKind, string> = {
    added: 'bg-success/10 text-success',
    changed: 'bg-warning/10 text-warning',
    removed: 'bg-error/10 text-error',
  };
  const LINE_BG: Record<string, string> = {
    add: 'bg-success/10 text-success',
    del: 'bg-error/10 text-error',
    meta: 'bg-info/5 text-base-content/50',
    context: 'text-base-content/80',
  };
  const GUTTER: Record<string, string> = { add: '+', del: '−', meta: '', context: ' ' };
</script>

{#if set.empty}
  <div class="rounded-lg border border-base-300 px-4 py-6 text-center text-xs text-base-content/60">
    {$t('settingsLayers.diffEmpty')}
  </div>
{:else}
  {#if counts.added > 0 || counts.removed > 0}
    <div class="flex items-center gap-3 mb-2 text-xs font-mono">
      <span class="text-success">+{counts.added}</span>
      <span class="text-error">&minus;{counts.removed}</span>
      <span class="text-base-content/50">
        {$t('settingsLayers.diffFiles', { values: { n: set.sections.length } })}
      </span>
    </div>
  {/if}

  <div class="flex flex-col gap-2">
    {#each set.sections as section, i (`${section.group ?? ''}/${section.title}#${i}`)}
      <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
        <div class="flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-200/50">
          <span class="text-xs font-mono truncate min-w-0">
            {#if section.group}<span class="text-base-content/50">{section.group}/</span>{/if}
            <span class="font-semibold">{section.title || $t('settingsLayers.diffUnnamed')}</span>
          </span>
          {#if section.kind}
            <span class="ml-auto shrink-0 px-1.5 py-0.5 rounded text-xs font-medium {KIND_BADGE[section.kind]}">
              {$t('settingsLayers.changeKind.' + section.kind)}
            </span>
          {/if}
        </div>
        <div class="overflow-x-auto py-1">
          {#each section.lines as line, li (li)}
            <div class="flex items-start font-mono text-xs leading-relaxed whitespace-pre min-w-max {LINE_BG[line.kind]}">
              <span class="w-4 shrink-0 select-none text-center opacity-60">{GUTTER[line.kind]}</span>
              <span class="pr-3">{line.text || ' '}</span>
            </div>
          {/each}
        </div>
      </div>
    {/each}
  </div>
{/if}
