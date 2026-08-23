<!--
  TranscriptMessage — ONE bubble for the hub-fed transcript surfaces
  (coworker threads, workrooms). ChatPane keeps its own richer message
  rendering (runs, tools, artifacts); these surfaces render a record of
  conversation, and this is that record's one shape.
-->
<script lang="ts">
  let {
    name,
    time = '',
    mine = false,
    html,
    initial = '',
    avatarClass = '',
  }: {
    name: string;
    time?: string;
    mine?: boolean;
    html: string;
    /** Optional colored avatar beside the name (multi-party rooms). */
    initial?: string;
    avatarClass?: string;
  } = $props();
</script>

<div class="flex flex-col {mine ? 'items-end' : 'items-start'}">
  <!-- The speaker's face rides the name line — small, in their roster color —
       so a multi-party room reads at a glance (owner-approved grammar). -->
  <div class="flex items-center gap-2 mb-1 {mine ? 'flex-row-reverse' : ''}">
    {#if initial}
      <span class="w-5 h-5 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {avatarClass || 'bg-base-200'}">{initial}</span>
    {/if}
    <span class="text-xs font-medium text-base-content/70">{name}</span>
    {#if time}<span class="text-xs text-base-content/40">{time}</span>{/if}
  </div>
  <div class="max-w-[85%] rounded-2xl px-4 py-2.5 text-sm leading-relaxed prose prose-sm {mine
    ? 'bg-primary/10 rounded-tr-sm'
    : 'bg-base-200 rounded-tl-sm'}">
    {@html html}
  </div>
</div>
