<script lang="ts">
  import { goto, withBase } from '$lib/nav';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sendClientEvent } from '$lib/api/gocliRequest';

  // The startup navigation failed twice: the spinner would sit forever with
  // nothing to say, so offer the composer as a plain link instead.
  let failed = $state(false);

  onMount(async () => {
    // Startup lands on the assistant's most recent conversation — the same
    // behavior as clicking its row (selectAgent opens the latest thread).
    // Landing on the blank new-chat composer made every launch look like the
    // existing conversation was gone. No chats yet → the composer is right.
    const started = Date.now();
    try {
      const api = await import('$lib/api/nebo');
      // The owner can make the Dashboard the front door (a user preference,
      // so every device agrees). A failed read falls through to the chat.
      const prefs = (await api.userGetPreferences().catch(() => null)) as { preferences?: { startPage?: string } | null } | null;
      if (prefs?.preferences?.startPage === 'dashboard') {
        await goto('/dashboard', { replaceState: true });
        return;
      }
      const r = await api.listAgentChats('assistant').catch((e: unknown) => {
        sendClientEvent('startup_chats_failed', { detail: String(e), durationMs: Date.now() - started });
        return null;
      });
      const latest = r?.chats?.[0]?.id;
      await goto(latest ? `/assistant/threads/${latest}` : '/assistant/threads', { replaceState: true });
    } catch (e) {
      sendClientEvent('startup_navigation_failed', { detail: String(e), durationMs: Date.now() - started });
      try {
        await goto('/assistant/threads', { replaceState: true });
      } catch (e2) {
        sendClientEvent('startup_fallback_failed', { detail: String(e2), durationMs: Date.now() - started });
        failed = true;
      }
    }
  });
</script>

<div class="flex-1 flex items-center justify-center h-full">
  {#if failed}
    <a class="link" href={withBase('/assistant/threads')}>{$t('layout.openAssistant')}</a>
  {:else}
    <span class="loading loading-spinner loading-md"></span>
  {/if}
</div>
