<script lang="ts">
	import { Chat } from '$lib/components/chat';
</script>

<svelte:head>
	<title>Nebo - Assistant</title>
</svelte:head>

{#key 'companion'}
	<Chat mode={{ type: 'companion' }} />
{/key}
