<!--
  The Inbox as a full page. Notification deep links land here, so it stays a
  real URL; the shelf opens the same view as a modal over the workspace. One
  component, two mounts — see $lib/components/inbox/InboxView.svelte.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import InboxView from '$lib/components/inbox/InboxView.svelte';

  // Selection lives in the URL (?m=<id>) so mobile gets a real screen: the OS
  // back gesture returns to the list. Desktop replaces the entry instead so
  // flipping through messages doesn't pile up history.
  const selectedId = $derived($page.url.searchParams.get('m'));
  const isDesktop = () => window.matchMedia('(min-width: 768px)').matches;

  function select(id: string | null) {
    if (id === null && !isDesktop()) { history.back(); return; }
    goto(id ? `/inbox?m=${encodeURIComponent(id)}` : '/inbox', {
      replaceState: isDesktop(),
      noScroll: true
    });
  }
</script>

<InboxView {selectedId} onselect={select} onnavigate={(link) => goto(link)} />
