<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Star } from 'lucide-svelte';
	import { formatRelative } from '$lib/time';

	interface Review {
		id?: string;
		rating: number;
		review?: string;
		body?: string;
		authorName?: string;
		reviewer_name?: string;
		createdAt?: string;
		created_at?: string;
	}

	interface Reply {
		id: string;
		review_id: string;
		body: string;
		created_at?: string;
		createdAt?: string;
	}

	let {
		review,
		reply = null
	}: {
		review: Review;
		reply?: Reply | null;
	} = $props();

	const reviewBody = $derived(review.review ?? review.body ?? '');
	const reviewerName = $derived(review.authorName ?? review.reviewer_name ?? $t('marketplace.detail.anonymous'));
	const createdAt = $derived(review.createdAt ?? review.created_at ?? '');

</script>

<div class="shrink-0 w-72 rounded-2xl bg-base-content/[0.04] p-4 flex flex-col">
	<div class="flex items-center justify-between mb-2">
		<span class="text-base font-semibold truncate">{reviewerName}</span>
		<span class="text-sm text-base-content/40 shrink-0 ml-2">{formatRelative(createdAt)}</span>
	</div>
	<div class="flex items-center gap-0.5 mb-2">
		{#each Array.from({ length: 5 }, (_, i) => i < review.rating) as filled}
			<Star class="w-3 h-3 {filled ? 'text-warning fill-warning' : 'text-base-content/15'}" />
		{/each}
	</div>
	<p class="text-base text-base-content/60 leading-relaxed line-clamp-4 flex-1">{reviewBody}</p>

	{#if reply}
		<div class="mt-3 pt-3 border-t border-base-content/10">
			<p class="text-sm font-semibold text-base-content/40 mb-1">{$t('marketplace.reviews.developerResponse')}</p>
			<p class="text-sm text-base-content/60 leading-relaxed line-clamp-3">{reply.body}</p>
		</div>
	{/if}
</div>
