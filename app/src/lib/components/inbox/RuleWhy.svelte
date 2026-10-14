<!--
  RuleWhy — "Why?" on something the assistant handled: the rule it acted
  under, when and where the owner said yes to it, and the two ways to change
  it (Edit, on the rules list; Stop doing this, which pauses it here).
-->
<script lang="ts">
  import { t, locale } from 'svelte-i18n';
  import { ruleById, rulesPath, setRulePaused, formatMoment, type LearnedRule } from '$lib/inbox/rules';
  import { surfaceKey } from '$lib/inbox/item';
  import { addToast } from '$lib/stores/toast';

  let { ruleId, assistantName, onnavigate }: { ruleId: string; assistantName: string; onnavigate: (link: string) => void } = $props();

  let rule = $state<LearnedRule | null | undefined>(undefined);
  let busy = $state(false);
  $effect(() => {
    const id = ruleId;
    ruleById(id).then((r) => (rule = r)).catch(() => (rule = null));
  });

  async function stop() {
    if (!rule || busy) return;
    busy = true;
    try {
      await setRulePaused(rule.id, true);
      rule = await ruleById(rule.id);
    } catch (e) {
      addToast((e as Error)?.message || $t('inbox.answerFailed'), 'error');
    } finally {
      busy = false;
    }
  }
</script>

<div class="inbox-why" data-testid="rule-why">
  {#if rule === undefined}
    <span class="loading loading-dots loading-xs text-base-content/40"></span>
  {:else if rule === null}
    <p>{$t('inbox.ruleGone')}</p>
  {:else}
    <p>{$t('inbox.yourRule', { values: { rule: rule.covers } })}{#if rule.does} {rule.does}{/if}</p>
    <p>
      {$t('inbox.youSaidYes', { values: { when: formatMoment(rule.consent, $locale ?? undefined) } })}{#if surfaceKey(rule.consent.via)}, {$t(surfaceKey(rule.consent.via)!)}{/if}
    </p>
    {#if rule.paused}
      <p class="font-medium">{$t('inbox.rulePaused', { values: { name: assistantName } })}</p>
    {/if}
    <div class="flex flex-wrap gap-1.5 mt-1">
      <button type="button" class="inbox-link" onclick={() => onnavigate(rulesPath())}>{$t('inbox.editRule')}</button>
      {#if !rule.paused}
        <button type="button" class="inbox-link inbox-link-danger" disabled={busy} onclick={stop}>{$t('inbox.stopRule')}</button>
      {/if}
    </div>
  {/if}
</div>
