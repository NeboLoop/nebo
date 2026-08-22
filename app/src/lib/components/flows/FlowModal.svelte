<!--
  Flow configuration — the structured parts a conversation shouldn't have to
  carry: what fires it, what it emits, and the HTTPS endpoint external systems
  post to.

  Authoring is chat. You ask your employee for what you want and it writes the
  binding. This is the fine-tuning, and the place the published endpoint lives:
  mint a key and an external form can trigger this flow directly.

  Every write is a single-field patch through updateAgentWorkflow, whose PUT is
  a merge — anything not touched here survives. It deliberately does NOT use the
  old whole-map save, which rewrote `activities` on every call and could flatten
  a multi-step flow to nothing.
-->
<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import * as api from '$lib/api/nebo';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import { getActivityType } from '$lib/utils/workflowTypes';
  import { triggerPayload } from '$lib/utils/workflowApi';
  import type { AgentPageContext, WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';
  import type { PublishAgentWorkflowResponse } from '$lib/api/neboComponents';

  let {
    name,
    onclose,
    onask,
    onopenrun
  }: {
    name: string | null;
    onclose: () => void;
    /** Hand a change of behaviour back to the employee — authoring is chat. */
    onask: (prompt: string) => void;
    onopenrun: (id: string) => void;
  } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const wf = $derived<WorkflowConfig | null>(
    name ? (ctx.workflowEntries.find(([k]) => k === name)?.[1] ?? null) : null
  );
  const runs = $derived(name ? ctx.runs.filter((r) => r.workflowName === name) : []);

  let notice = $state('');
  let busy = $state(false);

  const TRIGGERS = [
    { id: 'schedule', label: 'Schedule', icon: '⏱', hint: 'Runs on a repeating schedule.' },
    { id: 'heartbeat', label: 'Heartbeat', icon: '♥', hint: 'Runs on a fixed interval while active.' },
    { id: 'event', label: 'Event', icon: '⚡', hint: 'Runs when something else emits an event.' },
    { id: 'manual', label: 'Manual', icon: '▶', hint: 'Runs only when manually triggered.' }
  ];
  const triggerType = $derived(wf?.trigger?.type ?? 'manual');

  /** One patch, one field. The PUT merges, so nothing else is disturbed. */
  async function patch(body: Record<string, unknown>) {
    if (!name || busy) return;
    busy = true;
    notice = '';
    try {
      await api.updateAgentWorkflow(ctx.agentId, name, body);
      await ctx.refreshThreads?.();
    } catch (e) {
      notice = (e as Error).message;
    }
    busy = false;
  }

  function setTrigger(type: string) {
    if (!wf || type === triggerType) return;
    patch(triggerPayload({ ...wf, trigger: { ...(wf.trigger ?? {}), type } }));
  }

  async function testRun() {
    if (!name || busy) return;
    busy = true;
    notice = '';
    try {
      const res = await api.runAgentWorkflow(ctx.agentId, name);
      const id = (res as { run?: { id?: string } })?.run?.id;
      if (id) onopenrun(id);
      else notice = 'Started.';
    } catch (e) {
      notice = (e as Error).message;
    }
    busy = false;
  }

  async function remove() {
    if (!name || busy) return;
    busy = true;
    try {
      await api.deleteAgentWorkflow(ctx.agentId, name);
      onclose();
    } catch (e) {
      notice = (e as Error).message;
      busy = false;
    }
  }

  // ── Publish endpoint — mints a NeboLoop webhook (URL + one-time key) so
  // external callers can trigger this flow over HTTPS. Carried over from the
  // canvas verbatim; this is what wires up an external form.
  let publishing = $state(false);
  let publishError = $state('');
  let published = $state<PublishAgentWorkflowResponse | null>(null);
  let copiedField = $state('');

  const curlExample = $derived(
    published
      ? `curl -X POST ${published.url} -H "Authorization: Bearer ${published.key}" -H "Content-Type: application/json" -d '{"text":"..."}'`
      : ''
  );

  // The key is shown once — never carry one flow's result over to another.
  $effect(() => {
    void name;
    published = null;
    publishError = '';
  });

  async function publishEndpoint() {
    if (!name || publishing) return;
    publishing = true;
    publishError = '';
    try {
      published = await api.publishAgentWorkflow(ctx.agentId, name);
    } catch (e) {
      publishError = e instanceof Error ? e.message : 'Failed to publish endpoint';
    } finally {
      publishing = false;
    }
  }

  function copyText(field: string, text: string) {
    navigator.clipboard.writeText(text);
    copiedField = field;
    setTimeout(() => { copiedField = ''; }, 2000);
  }
</script>

<ShelfModal open={name !== null} title={name ?? ''} onclose={onclose}>
  <div class="flex-1 min-h-0 overflow-y-auto p-6">
    <div class="max-w-[560px] mx-auto flex flex-col gap-6">
      {#if !wf}
        <p class="text-sm text-base-content/50">This flow no longer exists.</p>
      {:else}
        <div class="flex items-center gap-2 flex-wrap">
          <label class="flex items-center gap-2 text-sm cursor-pointer">
            <input
              type="checkbox"
              class="toggle toggle-sm toggle-primary"
              checked={wf.isActive !== false}
              role="switch"
              aria-checked={wf.isActive !== false}
              onchange={() => name && ctx.toggleWorkflow(name)}
            />
            {wf.isActive === false ? $t('common.paused') : $t('common.active')}
          </label>
          <div class="flex-1"></div>
          <button class="btn btn-sm" onclick={testRun} disabled={busy}>Test run</button>
        </div>

        {#if notice}<p class="text-xs text-error">{notice}</p>{/if}

        <!-- Trigger -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Trigger</div>
          <div class="grid grid-cols-4 gap-2">
            {#each TRIGGERS as tr (tr.id)}
              <button
                class="flex flex-col items-center gap-1 py-2.5 rounded-lg border text-xs font-medium cursor-pointer transition-colors {triggerType === tr.id
                  ? 'border-primary bg-primary/10 text-primary'
                  : 'border-base-300 bg-base-100 hover:bg-base-200'}"
                onclick={() => setTrigger(tr.id)}
                disabled={busy}
              >
                <span class="text-base leading-none">{tr.icon}</span>
                {tr.label}
              </button>
            {/each}
          </div>
          <p class="text-xs text-base-content/60 mt-1.5">
            {TRIGGERS.find((x) => x.id === triggerType)?.hint}
          </p>
          {#if triggerType === 'schedule' && wf.schedule}
            <p class="text-sm font-mono mt-1.5">{wf.schedule}</p>
          {/if}
          {#if triggerType === 'event' && wf.trigger?.sources?.length}
            <p class="text-xs font-mono text-base-content/60 mt-1.5">{wf.trigger.sources.join(', ')}</p>
          {/if}
        </div>

        <!-- Description -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Description</div>
          <textarea
            class="w-full rounded-lg border border-base-300 bg-base-100 p-2.5 text-sm outline-none focus:border-primary min-h-[80px]"
            value={wf.description ?? ''}
            onchange={(e) => patch({ description: e.currentTarget.value })}
          ></textarea>
        </div>

        <!-- Emits -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Emits</div>
          <input
            type="text"
            class="w-full rounded-lg border border-base-300 bg-base-100 px-2.5 py-2 text-sm font-mono outline-none focus:border-primary"
            placeholder="e.g. {name}.complete"
            value={wf.emit ?? ''}
            onchange={(e) => patch({ emit: e.currentTarget.value })}
          />
          <p class="text-xs text-base-content/60 mt-1">
            Optional — other flows can trigger on this when the run completes.
          </p>
          {#if !wf.emit}
            <button
              class="text-xs text-primary hover:underline mt-1 bg-transparent border-none cursor-pointer p-0"
              onclick={() => patch({ emit: `${name}.complete` })}
            >Use {name}.complete</button>
          {/if}
        </div>

        <!-- Steps (authored in chat, shown here) -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Steps</div>
          {#if (wf.activities ?? []).length === 0}
            <p class="text-sm text-base-content/50">No steps yet.</p>
          {:else}
            <ol class="flex flex-col gap-1.5">
              {#each wf.activities ?? [] as a, i (a.id ?? i)}
                <li class="flex items-start gap-2.5 rounded-lg border border-base-300 bg-base-100 p-2.5">
                  <span class="text-sm shrink-0" title={getActivityType(a.type).label}>{getActivityType(a.type).icon}</span>
                  <span class="min-w-0">
                    <span class="block text-sm">{a.label || a.intent || a.type}</span>
                    {#if a.steps?.length}
                      <span class="block text-xs text-base-content/50 font-mono">{a.steps.length} steps</span>
                    {/if}
                  </span>
                </li>
              {/each}
            </ol>
          {/if}
          <button
            class="mt-2 w-full py-2 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors"
            onclick={() => { onclose(); onask(`Change the "${name}" flow: `); }}
          >Ask {ctx.agent?.name ?? 'your employee'} to change this</button>
        </div>

        <!-- Run history -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Run history</div>
          {#if runs.length === 0}
            <p class="text-sm text-base-content/50">{$t('agent.noRunsYet')}</p>
          {:else}
            <div class="flex flex-col">
              {#each runs.slice(0, 12) as r (r.id)}
                <button
                  type="button"
                  class="flex items-center gap-2.5 py-2 text-left border-0 border-b border-base-content/8 bg-transparent cursor-pointer hover:bg-base-200/60 transition-colors"
                  onclick={() => onopenrun(r.id)}
                >
                  <span class="w-2 h-2 rounded-full shrink-0 {r.status === 'success' ? 'bg-success' : r.status === 'running' ? 'bg-warning animate-pulse' : r.status === 'failed' ? 'bg-error' : 'bg-base-content/30'}"></span>
                  <span class="flex-1 text-sm font-mono">{r.dateGroup} &middot; {r.time}</span>
                  <span class="text-xs text-base-content/50 font-mono">{r.duration}</span>
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <!-- API endpoint (publish via NeboLoop) -->
        <div class="pt-4 border-t border-base-content/10">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">API endpoint</div>
          {#if published}
            <div class="mb-2">
              <div class="text-xs text-base-content/50 mb-0.5">URL</div>
              <div class="flex items-center gap-1.5">
                <code class="text-xs font-mono bg-base-200 rounded px-2 py-1 flex-1 min-w-0 truncate">{published.url}</code>
                <button class="btn btn-xs btn-ghost shrink-0" onclick={() => copyText('url', published?.url ?? '')}>{copiedField === 'url' ? 'Copied' : 'Copy'}</button>
              </div>
            </div>
            <div class="mb-2">
              <div class="text-xs text-base-content/50 mb-0.5">API key</div>
              <div class="flex items-center gap-1.5">
                <code class="text-xs font-mono bg-base-200 rounded px-2 py-1 flex-1 min-w-0 truncate">{published.key}</code>
                <button class="btn btn-xs btn-ghost shrink-0" onclick={() => copyText('key', published?.key ?? '')}>{copiedField === 'key' ? 'Copied' : 'Copy'}</button>
              </div>
              <div class="text-xs text-warning mt-1">Shown once — store it now.</div>
            </div>
            <div>
              <div class="text-xs text-base-content/50 mb-0.5">Example</div>
              <div class="flex items-start gap-1.5">
                <code class="text-xs font-mono bg-base-200 rounded px-2 py-1 flex-1 min-w-0 whitespace-pre-wrap break-all">{curlExample}</code>
                <button class="btn btn-xs btn-ghost shrink-0" onclick={() => copyText('curl', curlExample)}>{copiedField === 'curl' ? 'Copied' : 'Copy'}</button>
              </div>
            </div>
          {:else}
            <div class="text-xs text-base-content/60 mb-2">
              Mint a key so external systems — a web form, another app — can trigger this flow over HTTPS.
            </div>
            <button class="btn btn-sm btn-outline w-full" disabled={publishing} onclick={publishEndpoint}>
              {publishing ? 'Publishing…' : 'Publish endpoint'}
            </button>
            {#if publishError}<div class="text-xs text-error mt-1">{publishError}</div>{/if}
          {/if}
        </div>

        <button class="btn btn-sm btn-outline btn-error w-full" onclick={remove} disabled={busy}>
          Delete flow
        </button>
      {/if}
    </div>
  </div>
</ShelfModal>
