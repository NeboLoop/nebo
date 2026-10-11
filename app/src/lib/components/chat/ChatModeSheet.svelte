<!--
  The Permission sheet of the composer's + menu: the four modes one
  conversation can run in, one checked. Picking a mode closes the sheet; the
  composer saves it on the conversation. The employee's own setting is never
  touched here.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import { CHAT_MODES } from '$lib/chat/chatMode';

  interface Props {
    open: boolean;
    /** The mode the conversation runs in now. */
    selected: string;
    onpick: (mode: string) => void;
    onclose: () => void;
  }

  let { open, selected, onpick, onclose }: Props = $props();
</script>

<ShelfModal {open} title={$t('chatMode.title')} {onclose} narrow>
  <div class="chat-mode-body">
    <div class="chat-mode-list" role="radiogroup" aria-label={$t('chatMode.title')}>
      {#each CHAT_MODES as m (m.id)}
        <button
          type="button"
          class="chat-mode-row"
          role="radio"
          aria-checked={m.id === selected}
          data-mode={m.id}
          onclick={() => onpick(m.id)}
        >
          <span class="chat-mode-main">
            <span class="chat-mode-name">
              {$t(m.name)}
              {#if m.pill === 'recommended'}<span class="chat-mode-pill">{$t('chatMode.recommended')}</span>{/if}
              {#if m.pill === 'care'}<span class="chat-mode-pill chat-mode-pill-care">{$t('chatMode.useWithCare')}</span>{/if}
            </span>
            <span class="chat-mode-desc">{$t(m.desc)}</span>
          </span>
          <span class="chat-mode-check">{#if m.id === selected}<Check class="w-4 h-4" />{/if}</span>
        </button>
      {/each}
    </div>
    <p class="chat-mode-foot">{$t('chatMode.footnote')}</p>
  </div>
</ShelfModal>
