<script lang="ts">
  import { filterCommands, type SlashCommand } from './slashCommands.js';

  let { query = '', onselect, onclose }: {
    query?: string;
    onselect?: (cmd: SlashCommand) => void;
    onclose?: () => void;
  } = $props();

  const groups = $derived(filterCommands(query));
  const flat = $derived(groups.flatMap(g => g.items));

  let activeIdx = $state(0);

  // Reset index when results change
  $effect(() => {
    if (flat.length) activeIdx = 0;
  });

  function scrollSelectedIntoView() {
    requestAnimationFrame(() => {
      const el = document.querySelector('[data-slash-idx="' + activeIdx + '"]');
      if (el) el.scrollIntoView({ block: 'nearest' });
    });
  }

  export function handleKey(e: KeyboardEvent): boolean {
    if (!flat.length) return false;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      activeIdx = (activeIdx + 1) % flat.length;
      scrollSelectedIntoView();
      return true;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      activeIdx = (activeIdx - 1 + flat.length) % flat.length;
      scrollSelectedIntoView();
      return true;
    }
    if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault();
      onselect?.(flat[activeIdx]);
      return true;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose?.();
      return true;
    }
    return false;
  }
</script>

{#if flat.length > 0}
  <div class="absolute bottom-full left-0 right-0 mb-2 z-20 bg-base-100 border border-base-300 rounded-xl shadow-lg max-h-[320px] overflow-y-auto">
    {#each groups as group}
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-4 pt-3 pb-1">{group.category}</div>
      {#each group.items as cmd}
        {@const idx = flat.indexOf(cmd)}
        <button
          data-slash-idx={idx}
          class="flex items-start gap-3 px-4 py-2.5 w-full text-left cursor-pointer transition-colors border-none {idx === activeIdx ? 'bg-base-200' : 'bg-transparent hover:bg-base-200'}"
          onmouseenter={() => activeIdx = idx}
          onclick={() => onselect?.(cmd)}
        >
          <span class="text-xs font-semibold text-primary whitespace-nowrap">{cmd.name}</span>
          <span class="text-xs text-base-content/70">
            {cmd.desc}
            {#if cmd.args}
              <span class="font-mono text-base-content/50 ml-1">{cmd.args}</span>
            {/if}
          </span>
        </button>
      {/each}
    {/each}
  </div>
{/if}
