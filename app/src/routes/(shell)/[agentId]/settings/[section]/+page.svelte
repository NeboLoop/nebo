<!--
  Redirects /{agentId}/settings/{section} into the settings modal. Kept so
  every existing deep link and bookmark still lands on the right section.
  `/{agentId}/settings/accounts?plugin={slug}` (an Inbox item naming the
  plugin to connect) lands on the section that plugin's accounts live in.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import { accountsSectionFor } from '$lib/components/settings/agent/sections';

  $effect(() => {
    const id = $page.params.agentId;
    const plugin = $page.url.searchParams.get('plugin');
    const requested = $page.params.section || 'general';
    const section = requested === 'accounts' && plugin ? accountsSectionFor(plugin) : requested;
    if (!id) return;
    goto(`/${id}/threads?settings=${encodeURIComponent(section)}`, { replaceState: true });
  });
</script>
