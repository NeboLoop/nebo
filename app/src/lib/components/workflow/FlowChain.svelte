<!--
  A flow, read top to bottom.

  Replaces the freeform canvas as the way you READ a flow. The canvas rendered
  a chain as a thin horizontal ribbon that auto-fit to unreadable at any real
  length, and it had to draw a loop as an arrow going backwards — the hardest
  thing to read in any node graph.

  The engine's own model is containment, not back-edges: parser.rs::loop_body_set
  walks a loop's "Each item" edge and everything reachable from it, stopping at
  the back-edge into the loop. So a loop IS a box around the steps that repeat,
  and that is what this draws.

  Cards carry their content, because a step's meaning is its content: an agent
  step is its instruction, a code step is its source. "Run Code · 2 steps" tells
  a reader nothing.

  Read-only by design. Selection is reported upward; every write still goes
  through the config panel that already works.
-->
<script lang="ts">
  import { getActivityType } from '$lib/utils/workflowTypes';
  import type { WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';

  const TRIGGER_NODE = '__trigger__';
  const EMIT_NODE = '__emit__';

  let {
    workflow,
    selectedId = null,
    onselect
  }: {
    workflow: WorkflowConfig;
    selectedId?: string | null;
    onselect?: (id: string | null) => void;
  } = $props();

  const activities = $derived(workflow.activities ?? []);
  const connections = $derived(workflow.connections ?? []);
  const byId = $derived(new Map(activities.map((a) => [a.id, a])));

  /**
   * Mirrors parser.rs::loop_body_set — follow the loop's "Each item" edge, take
   * everything transitively reachable, stop at the loop itself (the back-edge)
   * and at the emit node.
   */
  function loopBody(loopId: string): Set<string> {
    const body = new Set<string>();
    const queue = connections
      .filter((c) => c.from === loopId && c.label === 'Each item')
      .map((c) => c.to);
    while (queue.length) {
      const node = queue.pop()!;
      if (node === loopId || node === EMIT_NODE || body.has(node)) continue;
      body.add(node);
      for (const c of connections.filter((c) => c.from === node)) queue.push(c.to);
    }
    return body;
  }

  type Row =
    | { kind: 'step'; activity: WorkflowActivity; depth: number }
    | { kind: 'loop-open'; activity: WorkflowActivity; depth: number }
    | { kind: 'loop-close'; depth: number }
    | { kind: 'branch'; activity: WorkflowActivity; depth: number; labels: string[] };

  /**
   * Linearise the graph for reading. With no connections the engine runs the
   * array in order, so we show it in order. With connections we walk from the
   * trigger, nesting loop bodies and marking branch points.
   */
  const rows = $derived.by((): Row[] => {
    const out: Row[] = [];
    if (activities.length === 0) return out;

    if (connections.length === 0) {
      for (const a of activities) out.push({ kind: 'step', activity: a, depth: 0 });
      return out;
    }

    const seen = new Set<string>();
    const walk = (id: string, depth: number) => {
      if (id === EMIT_NODE || seen.has(id)) return;
      const a = byId.get(id);
      if (!a) return;
      seen.add(id);

      const def = getActivityType(a.type);
      const outgoing = connections.filter((c) => c.from === id);

      if (a.type === 'loop') {
        const body = loopBody(id);
        out.push({ kind: 'loop-open', activity: a, depth });
        for (const c of outgoing.filter((c) => c.label === 'Each item')) walk(c.to, depth + 1);
        out.push({ kind: 'loop-close', depth });
        // Continue past the loop on its "Done" edge.
        for (const c of outgoing.filter((c) => c.label !== 'Each item')) {
          if (!body.has(c.to)) walk(c.to, depth);
        }
        return;
      }

      if (def.branches && outgoing.length > 1) {
        out.push({
          kind: 'branch',
          activity: a,
          depth,
          labels: outgoing.map((c) => c.label ?? '')
        });
        for (const c of outgoing) walk(c.to, depth + 1);
        return;
      }

      out.push({ kind: 'step', activity: a, depth });
      for (const c of outgoing) walk(c.to, depth);
    };

    const roots = connections.filter((c) => c.from === TRIGGER_NODE).map((c) => c.to);
    for (const r of roots) walk(r, 0);
    // Anything the walk never reached still deserves to be visible — an
    // orphaned step is a bug the reader should be able to see, not one we hide.
    for (const a of activities) if (!seen.has(a.id)) out.push({ kind: 'step', activity: a, depth: 0 });
    return out;
  });

  function triggerLine(): string {
    const tr = workflow.trigger;
    if (!tr || tr.type === 'manual') return 'Runs when you start it';
    if (tr.type === 'schedule') return workflow.schedule || 'On a schedule';
    if (tr.type === 'event') return `When ${tr.sources?.join(', ') || tr.event || 'an event'} fires`;
    if (tr.type === 'watch') return `Watching ${tr.event || tr.plugin || 'a plugin'}`;
    if (tr.type === 'heartbeat') return `Every ${tr.interval || '?'}`;
    return tr.type;
  }
  const triggerIcon = $derived(
    { schedule: '⏱', heartbeat: '♥', event: '⚡', watch: '👁', manual: '▶' }[
      workflow.trigger?.type ?? 'manual'
    ] ?? '▶'
  );

  /** A code step's meaning is its source, so show the source. */
  function codeOf(a: WorkflowActivity): { lang: string; src: string } | null {
    if (a.type !== 'code') return null;
    const p = (a.params ?? {}) as Record<string, unknown>;
    const src = typeof p.code === 'string' ? p.code : '';
    return { lang: typeof p.language === 'string' ? p.language : 'code', src };
  }

  /** Deterministic steps are guaranteed; agent steps are judgement. Say which. */
  const DETERMINISTIC = new Set(['code', 'tool', 'http', 'transform', 'connector', 'wait']);
</script>

<div class="flex flex-col items-stretch gap-0 p-5 max-w-[640px] mx-auto w-full">
  <!-- Trigger -->
  <div class="flex items-center gap-2.5 rounded-xl border border-primary/40 bg-primary/5 px-3.5 py-2.5">
    <span class="text-base leading-none">{triggerIcon}</span>
    <span class="text-sm font-medium">{triggerLine()}</span>
  </div>

  {#each rows as row, i (i)}
    {#if row.kind === 'loop-close'}
      <div style="margin-left: {row.depth * 20}px" class="border-l-2 border-b-2 border-warning/40 rounded-bl-xl h-3 ml-3"></div>
    {:else}
      <div class="w-px h-3 bg-base-content/20 self-start" style="margin-left: {row.depth * 20 + 18}px"></div>

      {#if row.kind === 'loop-open'}
        <div style="margin-left: {row.depth * 20}px" class="rounded-t-xl border-2 border-b-0 border-warning/40 bg-warning/5 px-3.5 py-2">
          <span class="text-sm font-medium">↻ For each {(row.activity.params?.items as string) || 'item'}</span>
          {#if row.activity.label}
            <span class="text-xs text-base-content/60 ml-1.5">{row.activity.label}</span>
          {/if}
        </div>
      {:else if row.kind === 'branch'}
        <button
          type="button"
          style="margin-left: {row.depth * 20}px"
          class="text-left rounded-xl border-2 px-3.5 py-2.5 cursor-pointer transition-colors {selectedId === row.activity.id
            ? 'border-info bg-info/10'
            : 'border-info/40 bg-info/5 hover:bg-info/10'}"
          onclick={() => onselect?.(row.activity.id)}
        >
          <span class="text-sm font-medium">◇ {row.activity.label || row.activity.id}</span>
          <span class="block text-xs text-base-content/60 mt-0.5">
            Splits into {row.labels.filter(Boolean).join(' / ') || `${row.labels.length} paths`}
          </span>
        </button>
      {:else}
        {@const a = row.activity}
        {@const def = getActivityType(a.type)}
        {@const code = codeOf(a)}
        <button
          type="button"
          style="margin-left: {row.depth * 20}px"
          class="text-left rounded-xl border px-3.5 py-2.5 cursor-pointer transition-colors {selectedId === a.id
            ? 'border-primary bg-primary/5 shadow-sm'
            : 'border-base-300 bg-base-100 hover:bg-base-200/60'}"
          onclick={() => onselect?.(a.id)}
        >
          <div class="flex items-center gap-2">
            <span class="text-sm shrink-0" title={def.label}>{def.icon}</span>
            <span class="text-sm font-medium truncate">{a.label || a.id}</span>
            <span class="text-[10px] uppercase tracking-wide px-1.5 py-px rounded {DETERMINISTIC.has(a.type)
              ? 'bg-base-200 text-base-content/60'
              : 'bg-primary/10 text-primary'}">
              {DETERMINISTIC.has(a.type) ? 'exact' : 'agent'}
            </span>
          </div>

          {#if code}
            <div class="mt-1.5 rounded-lg bg-base-200 overflow-hidden">
              <div class="px-2 py-0.5 text-[10px] font-mono text-base-content/50 border-b border-base-content/8">{code.lang}</div>
              <pre class="px-2 py-1.5 text-xs font-mono overflow-x-auto max-h-32">{code.src || '(empty)'}</pre>
            </div>
          {:else if a.intent || a.description}
            <p class="text-xs text-base-content/70 mt-1 line-clamp-3">{a.intent || a.description}</p>
          {/if}

          <div class="flex items-center gap-2 mt-1.5">
            {#if a.tool}<span class="text-[11px] font-mono text-base-content/50">{a.tool}</span>{/if}
            {#if a.steps?.length}
              <span class="text-[11px] font-mono text-base-content/40">{a.steps.length} steps</span>
            {/if}
          </div>
        </button>
      {/if}
    {/if}
  {/each}

  {#if workflow.emit}
    <div class="w-px h-3 bg-base-content/20 self-start ml-[18px]"></div>
    <div class="flex items-center gap-2.5 rounded-xl border border-accent/40 bg-accent/5 px-3.5 py-2.5">
      <span class="text-base leading-none">⚡</span>
      <span class="text-sm">emits <code class="font-mono text-accent">{workflow.emit}</code></span>
    </div>
  {/if}

  {#if rows.length === 0}
    <p class="text-center py-10 text-sm text-base-content/50">This flow has no steps yet.</p>
  {/if}
</div>
