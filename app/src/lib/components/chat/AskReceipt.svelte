<!--
  AskReceipt — an answered ask, collapsed in place to one muted line: how it
  ended, what was asked, and the answer ("Asked · What matters most… →
  Marketing", "Allowed · always · Send the invoice…"). No tint, border or
  card chrome: only an OPEN ask is a card. Tapping the line opens the
  question read-only (the children); tapping again folds it.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import ChevronRight from 'lucide-svelte/icons/chevron-right';

	interface Props {
		/** How it ended: "Asked", "Skipped", "Allowed · always", "Declined"… */
		lead: string;
		/** What was asked, on one line. */
		subject?: string;
		/** The answer given, when there is one to show. */
		answer?: string;
		/** The answer is a failure the owner should notice. */
		failed?: boolean;
		children: Snippet;
	}

	let { lead, subject = '', answer = '', failed = false, children }: Props = $props();
</script>

<details class="ask-receipt">
	<summary class="ask-receipt-line">
		<ChevronRight class="ask-receipt-chevron" />
		<span class="ask-receipt-lead">{lead}</span>
		{#if subject}<span class="ask-receipt-question">· {subject}</span>{/if}
		{#if answer}<span class="ask-receipt-answer" class:ask-receipt-failed={failed}>→ {answer}</span>{/if}
	</summary>
	<div class="ask-receipt-body">{@render children()}</div>
</details>
