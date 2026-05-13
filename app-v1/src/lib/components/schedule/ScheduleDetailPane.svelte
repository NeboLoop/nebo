<script>
  import { AGENTS, agentColor, triggerGlyph, fmtTime } from './data.js';

  let { item = null, onclose } = $props();

  const c = $derived(item ? agentColor(item.agent) : null);
  const agent = $derived(item ? AGENTS.find(x => x.id === item.agent) : null);
  const recentRuns = ['2:30 PM · 14s · ok', '1:30 PM · 17s · ok', '12:30 PM · 12s · ok'];
</script>

{#if !item}
  <div class="w-72 shrink-0 border-l border-base-content/10 bg-base-200/50 p-6 text-xs text-base-content/40 flex flex-col gap-2.5">
    <div class="font-mono text-[10px] uppercase tracking-wider">Detail</div>
    <div class="text-base-content/50 leading-relaxed">
      Click an event to see its trigger, agent, last run, and recent outputs.
    </div>
  </div>
{:else}
  <div class="w-72 shrink-0 border-l border-base-content/10 bg-base-100 flex flex-col overflow-auto">
    <div class="px-4 py-3 border-b border-base-content/5 flex items-start gap-2">
      <span class="font-mono text-xs opacity-75 mt-0.5 shrink-0" style:color={c.ink}>
        {triggerGlyph(item.kind)}
      </span>
      <div class="flex-1 min-w-0">
        <div class="text-sm font-semibold text-base-content leading-snug mb-1">{item.label}</div>
        <span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-[11px]" style="background:{c.bg}; color:{c.ink}">
          <span class="w-1.5 h-1.5 rounded-full" style:background={c.ink}></span>
          {agent.name}
        </span>
      </div>
      <button class="btn btn-ghost btn-xs text-base-content/40" onclick={onclose}>×</button>
    </div>
    <div class="p-4 flex flex-col gap-3.5">
      <div>
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider mb-0.5">When</div>
        <div class="text-xs text-base-content/70">{fmtTime(item.hour)} – {fmtTime(item.end)}</div>
      </div>
      <div>
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider mb-0.5">Trigger</div>
        <div class="text-xs text-base-content/70">
          {item.kind === 'sched' ? 'Scheduled · recurring' : item.kind === 'event' ? 'Event · webhook' : 'You · manual'}
        </div>
      </div>
      <div>
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider mb-0.5">Last run</div>
        <div class="text-xs font-mono text-base-content/70">2 min ago · ✓ ok</div>
      </div>
      <div>
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider mb-0.5">Avg duration</div>
        <div class="text-xs font-mono text-base-content/70">{Math.round(item.dur * 60)} min</div>
      </div>
      <div class="mt-1 pt-3 border-t border-base-content/5 flex flex-col gap-1.5">
        <div class="font-mono text-[10px] text-base-content/40 uppercase tracking-wider">Recent runs</div>
        {#each recentRuns as r}
          <div class="font-mono text-[11px] text-base-content/50">{r}</div>
        {/each}
      </div>
    </div>
  </div>
{/if}
