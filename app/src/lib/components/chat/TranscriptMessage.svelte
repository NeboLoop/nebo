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
  }: { name: string; time?: string; mine?: boolean; html: string } = $props();
</script>

<div class="flex flex-col {mine ? 'items-end' : 'items-start'}">
  <div class="flex items-baseline gap-2 mb-1 {mine ? 'flex-row-reverse' : ''}">
    <span class="text-xs font-medium text-base-content/70">{name}</span>
    {#if time}<span class="text-xs text-base-content/40">{time}</span>{/if}
  </div>
  <div class="max-w-[85%] rounded-2xl px-4 py-2.5 text-sm leading-relaxed prose prose-sm {mine
    ? 'bg-primary/10 rounded-tr-sm'
    : 'bg-base-200 rounded-tl-sm'}">
    {@html html}
  </div>
</div>
