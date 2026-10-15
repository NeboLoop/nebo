<!--
  Settings → Permissions: the company defaults every employee inherits, with
  the same controls as an employee's own Permissions page, and "Double-check
  before finishing" (`finishChecks` on GET/PUT /api/v1/agent/settings): before
  an employee ends a reply, it makes sure a save the owner asked for was saved
  and a command he gave was run, each with at most one more step.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import PermissionsSection from '$lib/components/settings/agent/PermissionsSection.svelte';
  import { getSettings, updateSettings } from '$lib/api/nebo';
  import { addToast } from '$lib/stores/toast';

  let finishChecks = $state<boolean | null>(null);
  let saving = $state(false);

  onMount(async () => {
    try {
      finishChecks = (await getSettings()).finishChecks !== false;
    } catch {
      addToast($t('permissions.loadError'), 'error');
    }
  });

  async function toggleFinishChecks(on: boolean) {
    saving = true;
    try {
      finishChecks = (await updateSettings({ finishChecks: on })).finishChecks !== false;
    } catch {
      addToast($t('permissions.saveError'), 'error');
    }
    saving = false;
  }
</script>

<SettingsHeader title={$t('permissions.title')} description={$t('permissions.companyDescription')} />

<PermissionsSection />

{#if finishChecks !== null}
  <div class="flex justify-between items-center gap-3 mt-6 p-4 rounded-xl border border-base-content/5 bg-base-100">
    <div class="flex flex-col gap-0.5 min-w-0">
      <span class="text-sm font-medium">{$t('permissions.finishChecksTitle')}</span>
      <span class="text-xs text-base-content/60">{$t('permissions.finishChecksDesc')}</span>
    </div>
    <input
      type="checkbox"
      class="toggle toggle-sm toggle-primary shrink-0"
      checked={finishChecks}
      disabled={saving}
      onchange={(e) => toggleFinishChecks((e.currentTarget as HTMLInputElement).checked)}
    />
  </div>
{/if}
