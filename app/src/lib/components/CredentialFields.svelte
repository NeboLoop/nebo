<!--
  CredentialFields — the values a plugin signs in with instead of a browser
  (a store domain and an access token, a mail server and a password), as its
  manifest declares them. The one form for them: an employee's settings and a
  chat's connect card both render it.
-->
<script module lang="ts">
  export type AuthField = { key: string; label: string; type: string; description: string; required: boolean };

  /** Whether every required field has a value. */
  export function credentialsComplete(fields: AuthField[], values: Record<string, string>): boolean {
    return fields.every((f) => !f.required || !!(values[f.key] ?? '').trim());
  }
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';

  let {
    fields,
    values = $bindable(),
    disabled = false,
    onenter
  }: {
    fields: AuthField[];
    values: Record<string, string>;
    disabled?: boolean;
    onenter?: () => void;
  } = $props();
</script>

{#each fields as field (field.key)}
  <label class="flex flex-col gap-1.5">
    <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{field.label}{#if !field.required} <span class="normal-case tracking-normal font-normal">({$t('common.optional')})</span>{/if}</span>
    <input
      type={field.type === 'password' ? 'password' : field.type === 'number' ? 'number' : 'text'}
      class="input input-sm input-bordered w-full text-sm font-body"
      autocomplete={field.type === 'password' ? 'current-password' : 'off'}
      bind:value={values[field.key]}
      {disabled}
      onkeydown={(e) => { if (e.key === 'Enter') onenter?.(); }}
    />
    {#if field.description}<span class="text-xs text-base-content/50">{field.description}</span>{/if}
  </label>
{/each}
