<!--
  ConsentChip — the consent line in the chat. When an employee drafts a new
  employee (or an edit that adds to a job), the create tool's result carries
  the one plain line of what it will be able to do. The owner's "yes" in the
  chat after this chip is the consent; there is no separate screen.
-->
<script lang="ts" module>
  export interface EmployeeConsentPayload {
    kind: 'employee_consent';
    draftId?: string;
    employee?: string;
    line?: string;
    adds?: boolean;
    [k: string]: unknown;
  }
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';

  let { consent }: { consent: EmployeeConsentPayload } = $props();
</script>

{#if consent.line}
  <div class="consent-chip">
    <p class="consent-line">{consent.line}</p>
    <p class="consent-chip-hint">{consent.adds ? $t('chat.consentAdds') : $t('chat.consentCreates')}</p>
  </div>
{/if}
