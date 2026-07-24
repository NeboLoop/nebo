<!--
  ApprovalControls — the per-employee Approvals section (Settings → employee →
  Approvals). Three-state control per gated operation, cloned from the MCP
  tool-permission pattern (Settings → MCP): Always allow / Needs approval /
  Blocked, grouped by capability, with an employee-wide default.

  Reads the aggregated view from GET /agents/{id}/operations; writes ride the
  ONE canonical pathway — the entity-config PUT `operationPolicy` patch. A
  critical (money/contract) operation is never loosened by the employee-wide
  default; it must be set to "Always allow" explicitly, per operation.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import Hand from 'lucide-svelte/icons/hand';
  import Ban from 'lucide-svelte/icons/ban';
  import ShieldAlert from 'lucide-svelte/icons/shield-alert';
  import * as api from '$lib/api/nebo';
  import { operationLabel, capabilityLabel } from '$lib/utils/operationLabels';
  import Spinner from '$lib/components/ui/Spinner.svelte';

  type Access = 'always' | 'approval' | 'blocked';

  interface OpRow {
    operation: string;
    capability: string;
    critical: boolean;
    override: Access | null;
    effective: Access;
  }

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let defaultAccess = $state<Access>('approval');
  let rows = $state<OpRow[]>([]);
  let loadError = $state('');

  const groups = $derived.by(() => {
    const byCap = new Map<string, OpRow[]>();
    for (const r of rows) {
      const list = byCap.get(r.capability) ?? [];
      list.push(r);
      byCap.set(r.capability, list);
    }
    return [...byCap.entries()];
  });

  // Selected = a solid color-filled chip; unselected = faint ghost. The owner
  // must see the active state at a glance (icon tint alone was too subtle).
  const accessStates: { value: Access; labelKey: string; icon: typeof Check; active: string }[] = [
    {
      value: 'always',
      labelKey: 'agentApprovals.alwaysAllow',
      icon: Check,
      active: 'bg-success/15 border-success/40 text-success',
    },
    {
      value: 'approval',
      labelKey: 'agentApprovals.needsApproval',
      icon: Hand,
      active: 'bg-warning/15 border-warning/40 text-warning',
    },
    {
      value: 'blocked',
      labelKey: 'agentApprovals.blocked',
      icon: Ban,
      active: 'bg-error/15 border-error/40 text-error',
    },
  ];

  function stateLabelKey(v: Access): string {
    return accessStates.find((s) => s.value === v)?.labelKey ?? '';
  }

  async function load() {
    loading = true;
    loadError = '';
    try {
      const resp = (await api.getAgentOperations(agentId)) as unknown as {
        default: Access;
        operations: OpRow[];
      };
      defaultAccess = resp.default;
      rows = resp.operations;
    } catch {
      loadError = $t('agentApprovals.loadError');
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (agentId) load();
  });

  async function persist() {
    saving = true;
    try {
      const operations: Record<string, Access> = {};
      for (const r of rows) if (r.override) operations[opSuffix(r.operation)] = r.override;
      await api.updateEntityConfig('agent', agentId, {
        operationPolicy: JSON.stringify({ default: defaultAccess, operations }),
      });
      await load();
    } finally {
      saving = false;
    }
  }

  function opSuffix(operation: string): string {
    const parts = operation.split('.');
    return parts.length > 3 ? parts.slice(-3).join('.') : operation;
  }

  function setRowState(row: OpRow, value: Access) {
    // Clicking the already-explicit state clears the override (back to default) —
    // same interaction as the MCP tool-permission rows.
    row.override = row.override === value ? null : value;
    persist();
  }

  function setDefault(value: Access) {
    defaultAccess = value;
    persist();
  }
</script>

<div class="max-w-2xl">
  <p class="text-sm text-base-content/70 mb-4">{$t('agentApprovals.intro')}</p>

  {#if loading}
    <div class="flex justify-center py-10"><Spinner /></div>
  {:else if loadError}
    <p class="text-sm text-error">{loadError}</p>
  {:else if rows.length === 0}
    <p class="text-sm text-base-content/50">{$t('agentApprovals.noOperations')}</p>
  {:else}
    <!-- Employee-wide default -->
    <div class="flex items-center justify-between gap-4 rounded-xl bg-base-200/50 border border-base-content/10 px-4 py-3 mb-5">
      <div>
        <div class="text-sm font-medium text-base-content">{$t('agentApprovals.defaultLabel')}</div>
        <div class="text-xs text-base-content/60">{$t('agentApprovals.defaultHint')}</div>
      </div>
      <select
        class="select select-sm select-bordered"
        value={defaultAccess}
        onchange={(e) => setDefault((e.currentTarget as HTMLSelectElement).value as Access)}
        disabled={saving}
      >
        {#each accessStates as s (s.value)}
          <option value={s.value}>{$t(s.labelKey)}</option>
        {/each}
      </select>
    </div>

    {#each groups as [capability, ops] (capability)}
      <div class="mb-5">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">
          {capabilityLabel(capability)}
        </div>
        <div class="rounded-xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/5">
          {#each ops as row (row.operation)}
            <div class="flex items-center justify-between gap-3 px-4 py-2.5">
              <div class="min-w-0">
                <div class="flex items-center gap-1.5">
                  <span class="text-sm font-medium text-base-content truncate">{operationLabel(row.operation)}</span>
                  {#if row.critical}
                    <span class="inline-flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-accent" title={$t('agentApprovals.criticalHint')}>
                      <ShieldAlert class="w-3 h-3" />{$t('agentApprovals.critical')}
                    </span>
                  {/if}
                </div>
                <div class="text-xs text-base-content/50">
                  {$t(stateLabelKey(row.effective))}{#if !row.override}
                    · {$t('agentApprovals.usingDefault')}{/if}
                </div>
              </div>
              <div class="join shrink-0">
                {#each accessStates as s (s.value)}
                  <button
                    class="join-item px-2.5 py-1.5 border transition-colors cursor-pointer {row.effective === s.value
                      ? s.active
                      : 'bg-base-100 border-base-content/10 text-base-content/25 hover:text-base-content/60 hover:bg-base-200'}"
                    title={$t(s.labelKey)}
                    aria-label={$t(s.labelKey)}
                    aria-pressed={row.effective === s.value}
                    disabled={saving}
                    onclick={() => setRowState(row, s.value)}
                  >
                    <s.icon class="w-3.5 h-3.5" />
                  </button>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/each}

    <p class="text-xs text-base-content/50 mt-2">{$t('agentApprovals.criticalFootnote')}</p>
  {/if}
</div>
