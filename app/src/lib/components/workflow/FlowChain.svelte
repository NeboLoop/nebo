<!--
  A flow, read and edited top to bottom. The only view — the freeform canvas
  rendered a chain as an unreadable ribbon and drew loops as back-edges.

  Editing is two-speed, on the card itself:
  - The card's main field is a form: an agent step's instruction and a code
    step's source edit inline, writing through the builder's one update path.
  - The ✨ button opens a single-line AI input — say what you want, the ops
    apply to the draft, no conversation to manage. (This replaced the
    Architect, which was the same engine behind a second chat pane.)

  The strip above the chain shows what the flow needs to be CONNECTED to —
  derived from the steps' own tool prefixes — and whether it is, with the fix
  one click away. A flow that looks finished but isn't plugged in is the
  silent-failure case this exists to prevent.

  Loop semantics mirror parser.rs::loop_body_set: a loop OWNS its body (walk
  the "Each item" edge, stop at the back-edge), so a loop draws as a box
  around the steps that repeat. With no connections the engine runs the array
  in order, so the chain shows it in order.
-->
<script lang="ts">
  import { getActivityType } from '$lib/utils/workflowTypes';
  import { describeSchedule } from '$lib/utils/schedule';
  import type { WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';

  const TRIGGER_NODE = '__trigger__';
  const EMIT_NODE = '__emit__';

  type FlowConnection = {
    slug: string;
    name: string;
    connected: boolean;
    section: 'accounts' | 'channels';
  };

  let {
    workflow,
    selectedId = null,
    editable = false,
    aiBusy = null,
    aiNote = '',
    connections = null,
    onselect,
    onupdate,
    onaiedit,
    onaddstep,
    onconnect
  }: {
    workflow: WorkflowConfig;
    selectedId?: string | null;
    editable?: boolean;
    /** Activity id (or '__workflow__') an AI edit is running against. */
    aiBusy?: string | null;
    /** Outcome of the last AI edit, shown once under the flow input. */
    aiNote?: string;
    connections?: FlowConnection[] | null;
    onselect?: (id: string | null) => void;
    onupdate?: (id: string, field: keyof WorkflowActivity, value: unknown) => void;
    onaiedit?: (id: string | null, instruction: string) => void;
    onaddstep?: (afterId: string | null) => void;
    onconnect?: (section: string) => void;
  } = $props();

  const activities = $derived(workflow.activities ?? []);
  const connectionsList = $derived(workflow.connections ?? []);
  const byId = $derived(new Map(activities.map((a) => [a.id, a])));

  // ── AI input state: which card's input is open, and its draft text.
  let aiOpenFor = $state<string | null>(null);
  let aiText = $state('');

  function submitAi(target: string | null) {
    const text = aiText.trim();
    if (!text) return;
    onaiedit?.(target, text);
    aiText = '';
    aiOpenFor = null;
  }

  /** Mirrors parser.rs::loop_body_set. */
  function loopBody(loopId: string): Set<string> {
    const body = new Set<string>();
    const queue = connectionsList
      .filter((c) => c.from === loopId && c.label === 'Each item')
      .map((c) => c.to);
    while (queue.length) {
      const node = queue.pop()!;
      if (node === loopId || node === EMIT_NODE || body.has(node)) continue;
      body.add(node);
      for (const c of connectionsList.filter((c) => c.from === node)) queue.push(c.to);
    }
    return body;
  }

  /**
   * The chain as a tree: a loop node OWNS its body (exactly the engine's
   * model — parser.rs::loop_body_set), a branch owns one list per labelled
   * path. Rendering is then a recursive walk, and a loop's steps sit inside
   * its container instead of being bracketed by open/close markers.
   */
  type ChainNode =
    | { kind: 'step'; a: WorkflowActivity }
    | { kind: 'loop'; a: WorkflowActivity; body: ChainNode[] }
    | { kind: 'branch'; a: WorkflowActivity; paths: { label: string; nodes: ChainNode[] }[] };

  const tree = $derived.by((): ChainNode[] => {
    if (activities.length === 0) return [];
    // No connections → the engine runs the array in order; show it in order.
    if (connectionsList.length === 0) {
      return activities.map((a) => ({ kind: 'step', a }) as ChainNode);
    }

    const seen = new Set<string>();
    const walk = (id: string): ChainNode[] => {
      if (id === EMIT_NODE || seen.has(id)) return [];
      const a = byId.get(id);
      if (!a) return [];
      seen.add(id);

      const def = getActivityType(a.type);
      const outgoing = connectionsList.filter((c) => c.from === id);

      if (a.type === 'loop') {
        const bodySet = loopBody(id);
        const body = outgoing
          .filter((c) => c.label === 'Each item')
          .flatMap((c) => walk(c.to));
        const after = outgoing
          .filter((c) => c.label !== 'Each item' && !bodySet.has(c.to))
          .flatMap((c) => walk(c.to));
        return [{ kind: 'loop', a, body }, ...after];
      }

      if (def.branches && outgoing.length > 1) {
        const paths = outgoing.map((c) => ({ label: c.label ?? '', nodes: walk(c.to) }));
        return [{ kind: 'branch', a, paths }];
      }

      return [{ kind: 'step', a } as ChainNode, ...outgoing.flatMap((c) => walk(c.to))];
    };

    const out = connectionsList
      .filter((c) => c.from === TRIGGER_NODE)
      .flatMap((c) => walk(c.to));
    // Orphans stay visible — an unreachable step is a bug the reader should
    // see, not one we hide.
    for (const a of activities) if (!seen.has(a.id)) out.push({ kind: 'step', a });
    return out;
  });

  function triggerLine(): string {
    const tr = workflow.trigger;
    if (!tr || tr.type === 'manual') return 'Runs when you start it';
    if (tr.type === 'schedule') {
      const raw = workflow.schedule || tr.cron || '';
      return raw ? describeSchedule(raw).text : 'On a schedule';
    }
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

  function codeOf(a: WorkflowActivity): { lang: string; src: string } | null {
    if (a.type !== 'code') return null;
    const p = (a.params ?? {}) as Record<string, unknown>;
    return {
      lang: typeof p.language === 'string' ? p.language : 'code',
      src: typeof p.code === 'string' ? p.code : ''
    };
  }

  function setCode(a: WorkflowActivity, src: string) {
    onupdate?.(a.id, 'params', { ...(a.params ?? {}), code: src });
  }

  const DETERMINISTIC = new Set(['code', 'tool', 'http', 'transform', 'connector', 'wait']);

  /** The last id in reading order — the `+` after the chain inserts after it. */
  const lastStepId = $derived.by(() => {
    const last = tree[tree.length - 1];
    return last ? last.a.id : null;
  });
</script>

{#snippet aiInput(target: string | null, placeholder: string)}
  {#if aiBusy === (target ?? '__workflow__')}
    <div class="flex items-center gap-2 mt-1.5 text-xs text-base-content/60">
      <span class="loading loading-spinner loading-xs"></span> Applying…
    </div>
  {:else if aiOpenFor === (target ?? '__top__')}
    <form
      class="flex items-center gap-1.5 mt-1.5"
      onsubmit={(e) => { e.preventDefault(); submitAi(target); }}
    >
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        bind:value={aiText}
        {placeholder}
        autofocus
        class="flex-1 min-w-0 h-8 px-2.5 rounded-field border border-primary/40 bg-base-100 text-sm outline-none focus:border-primary placeholder:text-base-content/40"
        onkeydown={(e) => { if (e.key === 'Escape') { aiOpenFor = null; aiText = ''; } }}
      />
      <button type="submit" class="btn btn-sm btn-primary" disabled={!aiText.trim()}>Go</button>
    </form>
  {/if}
{/snippet}

{#snippet aiButton(target: string | null)}
  {#if editable && onaiedit}
    <button
      type="button"
      class="w-6 h-6 rounded flex items-center justify-center shrink-0 bg-transparent border-none cursor-pointer text-base-content/40 hover:text-primary hover:bg-primary/10 transition-colors"
      title="Tell the AI what to change"
      onclick={(e) => {
        e.stopPropagation();
        const key = target ?? '__top__';
        aiOpenFor = aiOpenFor === key ? null : key;
        aiText = '';
      }}
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M18.4 5.6l-2.1 2.1M7.7 16.3l-2.1 2.1"/><circle cx="12" cy="12" r="3.5"/></svg>
    </button>
  {/if}
{/snippet}

{#snippet stepCard(a: WorkflowActivity)}
  {@const def = getActivityType(a.type)}
  {@const code = codeOf(a)}
  {@const selected = selectedId === a.id}
  <div
    class="rounded-xl border transition-colors {selected
      ? 'border-primary bg-primary/5 shadow-sm'
      : 'border-base-300 bg-base-100 hover:bg-base-200/60'}"
  >
    <button
      type="button"
      class="w-full flex items-center gap-2 px-3.5 pt-2.5 text-left bg-transparent border-none cursor-pointer"
      onclick={() => onselect?.(a.id)}
    >
      <span class="text-sm shrink-0" title={def.label}>{def.icon}</span>
      <span class="text-sm font-medium truncate flex-1">{a.label || a.id}</span>
      <span class="text-[10px] uppercase tracking-wide px-1.5 py-px rounded shrink-0 {DETERMINISTIC.has(a.type)
        ? 'bg-base-200 text-base-content/60'
        : 'bg-primary/10 text-primary'}">
        {DETERMINISTIC.has(a.type) ? 'exact' : 'agent'}
      </span>
      {@render aiButton(a.id)}
    </button>

    <div class="px-3.5 pb-2.5">
      {#if code}
        {#if editable && selected}
          <div class="mt-1.5 rounded-lg bg-base-200 overflow-hidden">
            <div class="px-2 py-0.5 text-[10px] font-mono text-base-content/50 border-b border-base-content/8">{code.lang}</div>
            <textarea
              class="w-full px-2 py-1.5 text-xs font-mono bg-transparent outline-none resize-y min-h-[96px]"
              value={code.src}
              onchange={(e) => setCode(a, e.currentTarget.value)}
              spellcheck="false"
            ></textarea>
          </div>
        {:else}
          <div class="mt-1.5 rounded-lg bg-base-200 overflow-hidden">
            <div class="px-2 py-0.5 text-[10px] font-mono text-base-content/50 border-b border-base-content/8">{code.lang}</div>
            <pre class="px-2 py-1.5 text-xs font-mono overflow-x-auto max-h-32">{code.src || '(empty)'}</pre>
          </div>
        {/if}
      {:else if editable && selected}
        <!-- The card IS the form: the step's instruction edits in place. -->
        <textarea
          class="w-full mt-1.5 px-2.5 py-2 text-xs rounded-lg border border-base-300 bg-base-100 outline-none focus:border-primary resize-y min-h-[64px]"
          placeholder="What should this step do?"
          value={a.intent || a.description || ''}
          onchange={(e) => onupdate?.(a.id, 'intent', e.currentTarget.value)}
        ></textarea>
      {:else if a.intent || a.description}
        <p class="text-xs text-base-content/70 mt-1 line-clamp-3">{a.intent || a.description}</p>
      {/if}

      {#if a.tool || a.steps?.length}
        <div class="flex items-center gap-2 mt-1.5">
          {#if a.tool}<span class="text-[11px] font-mono text-base-content/50">{a.tool}</span>{/if}
          {#if a.steps?.length}
            <span class="text-[11px] font-mono text-base-content/40">{a.steps.length} steps</span>
          {/if}
        </div>
      {/if}

      {@render aiInput(a.id, `Change "${a.label || a.id}"…`)}
    </div>
  </div>
{/snippet}

{#snippet chainList(nodes: ChainNode[])}
  {#each nodes as node, i (node.a.id)}
    {@render insertButton(i > 0 ? nodes[i - 1].a.id : null)}

    {#if node.kind === 'loop'}
      <!-- The body sits INSIDE the loop's container — the box is the loop. -->
      <div class="rounded-xl border-2 border-warning/40 bg-warning/5 overflow-hidden">
        <div class="px-3.5 py-2 border-b border-warning/30 flex items-center gap-2">
          <span class="text-sm font-medium">↻ For each {(node.a.params?.items as string) || 'item'}</span>
          {#if node.a.label && node.a.label !== node.a.id}
            <span class="text-xs text-base-content/60">{node.a.label}</span>
          {/if}
          <span class="flex-1"></span>
          {@render aiButton(node.a.id)}
        </div>
        <div class="p-3 flex flex-col items-stretch">
          {#if node.body.length === 0}
            <p class="text-xs text-base-content/50 text-center py-3">Nothing in this loop yet.</p>
          {:else}
            {@render chainList(node.body)}
          {/if}
        </div>
      </div>
    {:else if node.kind === 'branch'}
      <button
        type="button"
        class="text-left rounded-xl border-2 px-3.5 py-2.5 cursor-pointer transition-colors {selectedId === node.a.id
          ? 'border-info bg-info/10'
          : 'border-info/40 bg-info/5 hover:bg-info/10'}"
        onclick={() => onselect?.(node.a.id)}
      >
        <span class="text-sm font-medium">◇ {node.a.label || node.a.id}</span>
      </button>
      {#each node.paths as path (path.label)}
        <div class="mt-2 ml-5 rounded-xl border border-info/30 overflow-hidden">
          <div class="px-3 py-1.5 bg-info/5 border-b border-info/20 text-xs font-medium text-info">{path.label || 'path'}</div>
          <div class="p-3 flex flex-col items-stretch">
            {#if path.nodes.length === 0}
              <p class="text-xs text-base-content/50 text-center py-2">Empty path</p>
            {:else}
              {@render chainList(path.nodes)}
            {/if}
          </div>
        </div>
      {/each}
    {:else}
      {@render stepCard(node.a)}
    {/if}
  {/each}
{/snippet}

{#snippet insertButton(afterId: string | null)}
  {#if editable && onaddstep}
    <div class="self-start flex items-center" style="margin-left: 12px">
      <div class="w-px h-3 bg-base-content/20"></div>
      <button
        type="button"
        class="ml-[-6px] w-4 h-4 rounded-full border border-base-300 bg-base-100 text-base-content/40 hover:text-primary hover:border-primary flex items-center justify-center cursor-pointer text-[10px] leading-none transition-colors"
        title="Add a step here"
        onclick={() => onaddstep?.(afterId)}
      >+</button>
    </div>
  {:else}
    <div class="w-px h-3 bg-base-content/20 self-start ml-[18px]"></div>
  {/if}
{/snippet}

<div class="flex flex-col items-stretch gap-0 p-5 max-w-[640px] mx-auto w-full">
  <!-- What this flow must be connected to. -->
  {#if connections && connections.length > 0}
    <div class="flex items-center gap-1.5 flex-wrap mb-3">
      {#each connections as c (c.slug)}
        <button
          type="button"
          class="inline-flex items-center gap-1.5 px-2 py-1 rounded-full border text-xs cursor-pointer transition-colors {c.connected
            ? 'border-success/40 bg-success/5 text-base-content/70'
            : 'border-error/50 bg-error/5 text-error hover:bg-error/10'}"
          title={c.connected ? `${c.name} is connected` : `${c.name} is not connected — this flow will fail`}
          onclick={() => onconnect?.(c.section)}
        >
          <span class="w-1.5 h-1.5 rounded-full {c.connected ? 'bg-success' : 'bg-error'}"></span>
          {c.name}
          {#if !c.connected}<span class="font-medium">· Connect</span>{/if}
        </button>
      {/each}
    </div>
  {/if}

  <!-- Flow-level AI input -->
  {#if editable && onaiedit}
    <div class="mb-3">
      {#if aiOpenFor === '__top__' || aiBusy === '__workflow__'}
        {@render aiInput(null, 'Describe a change to this whole flow…')}
      {:else}
        <button
          type="button"
          class="w-full flex items-center gap-2 px-3 py-2 rounded-field border border-dashed border-base-300 text-sm text-base-content/50 hover:text-primary hover:border-primary/40 cursor-pointer bg-transparent transition-colors"
          onclick={() => { aiOpenFor = '__top__'; aiText = ''; }}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M18.4 5.6l-2.1 2.1M7.7 16.3l-2.1 2.1"/><circle cx="12" cy="12" r="3.5"/></svg>
          Tell the AI what to change…
        </button>
      {/if}
      {#if aiNote}
        <p class="text-xs text-base-content/60 mt-1.5">{aiNote}</p>
      {/if}
    </div>
  {/if}

  <!-- Trigger -->
  <button
    type="button"
    class="flex items-center gap-2.5 rounded-xl border px-3.5 py-2.5 text-left cursor-pointer transition-colors {selectedId === TRIGGER_NODE
      ? 'border-primary bg-primary/10'
      : 'border-primary/40 bg-primary/5 hover:bg-primary/10'}"
    onclick={() => onselect?.(TRIGGER_NODE)}
  >
    <span class="text-base leading-none">{triggerIcon}</span>
    <span class="text-sm font-medium flex-1">{triggerLine()}</span>
    {#if editable}
      <span class="text-xs text-base-content/40">edit</span>
    {/if}
  </button>

  {@render chainList(tree)}

  {@render insertButton(lastStepId)}

  {#if workflow.emit}
    <div class="flex items-center gap-2.5 rounded-xl border border-accent/40 bg-accent/5 px-3.5 py-2.5">
      <span class="text-base leading-none">⚡</span>
      <span class="text-sm">emits <code class="font-mono text-accent">{workflow.emit}</code></span>
    </div>
  {/if}

  {#if tree.length === 0}
    <p class="text-center py-10 text-sm text-base-content/50">This flow has no steps yet.</p>
  {/if}
</div>
