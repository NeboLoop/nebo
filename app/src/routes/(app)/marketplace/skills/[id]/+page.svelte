<script lang="ts">
	import { ArrowLeft, Star, X, Check, Copy } from 'lucide-svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import webapi from '$lib/api/gocliRequest';
	import MediaGallery from '$lib/components/marketplace/MediaGallery.svelte';
	import ReviewCard from '$lib/components/marketplace/ReviewCard.svelte';
	import FeedbackSection from '$lib/components/marketplace/FeedbackSection.svelte';

	let showReview = $state(false);
	let reviewRating = $state(5);
	let reviewText = $state('');
	let codeCopied = $state(false);

	import { type AppItem, toAppItem, itemHref } from '$lib/types/marketplace';
	import InstallCode from '$lib/components/InstallCode.svelte';

	let skill: any = $state(null);
	let reviews: any[] = $state([]);
	let reviewReplies: any[] = $state([]);
	let feedbackItems: any[] = $state([]);
	let screenshots: any[] = $state([]);
	let similarItems: AppItem[] = $state([]);
	let loading = $state(true);
	let submittingReview = $state(false);
	let installing = $state(false);

	const skillId = $derived($page.params.id);

	const replyMap = $derived<Record<string, any>>(() => {
		const map: Record<string, any> = {};
		for (const rr of reviewReplies) {
			map[rr.review_id] = rr;
		}
		return map;
	});

	onMount(async () => {
		try {
			const [skillRes, reviewsRes, mediaRes, feedbackRes] = await Promise.all([
				webapi.get<any>(`/api/v1/store/products/${skillId}`),
				webapi.get<any>(`/api/v1/store/products/${skillId}/reviews`).catch(() => ({ reviews: [] })),
				webapi.get<any>(`/api/v1/store/products/${skillId}/media`).catch(() => ({ media: [] })),
				webapi.get<any>(`/api/v1/store/products/${skillId}/feedback`).catch(() => ({ feedback: [] }))
			]);
			skill = skillRes;
			reviews = reviewsRes.reviews ?? [];
			screenshots = mediaRes.media ?? [];
			feedbackItems = feedbackRes.feedback ?? [];
		} catch {
			skill = null;
		}
		loading = false;
		webapi.get<any>(`/api/v1/store/products/${skillId}/similar`).then((res: any) => {
			similarItems = (res.apps || []).map((a: any, i: number) => toAppItem(a, i));
		}).catch(() => {});
	});

	async function submitReview() {
		submittingReview = true;
		try {
			await webapi.post(`/api/v1/store/products/${skillId}/reviews`, { rating: reviewRating, body: reviewText });
			showReview = false;
			reviewText = '';
			const res = await webapi.get<any>(`/api/v1/store/products/${skillId}/reviews`);
			reviews = res.reviews ?? [];
		} catch { /* ignore */ }
		submittingReview = false;
	}

	async function installProduct() {
		installing = true;
		try {
			await webapi.post(`/api/v1/store/products/${skillId}/install`);
		} catch { /* ignore */ }
		installing = false;
	}

	async function copyCode() {
		if (!skill?.code) return;
		await navigator.clipboard.writeText(skill.code);
		codeCopied = true;
		setTimeout(() => codeCopied = false, 2000);
	}

	function formatNumber(n: number) {
		if (!n) return '0';
		if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
		return n.toString();
	}

	function renderStars(rating: number) {
		return Array.from({ length: 5 }, (_, i) => i < rating);
	}

	const avgRating = $derived(skill?.ratingAvg && Number(skill.ratingAvg) > 0 ? Number(skill.ratingAvg).toFixed(1) : null);
</script>

<!-- Header -->
<div class="sticky top-0 z-20 bg-base-100/80 backdrop-blur-xl border-b border-base-content/5">
	<div class="flex items-center gap-3 px-5 h-14">
		<button type="button" onclick={() => history.back()} class="p-1.5 -ml-1.5 rounded-full hover:bg-base-content/5 transition-colors">
			<ArrowLeft class="w-5 h-5" />
		</button>
		{#if skill}
			<span class="text-base font-medium text-base-content/80 truncate">{skill.name}</span>
		{/if}
	</div>
</div>

<div class="max-w-7xl mx-auto">
{#if loading}
	<div class="flex justify-center py-24">
		<span class="loading loading-spinner loading-md text-primary"></span>
	</div>
{:else if !skill}
	<div class="flex flex-col items-center justify-center py-24 text-center">
		<p class="text-base text-base-content/80">Skill not found</p>
	</div>
{:else}
	<!-- Hero: icon + name + install button -->
	<div class="px-5 pt-6 pb-5">
		<div class="flex items-start gap-5">
			<div class="w-28 h-28 rounded-[22px] bg-gradient-to-br from-base-content/5 to-base-content/10 flex items-center justify-center shrink-0">
				{#if skill.icon}
					<img src={skill.icon} alt="" class="w-28 h-28 rounded-[22px]" />
				{:else}
					<img src="/images/default-skill.svg" alt="" class="w-20 h-20" />
				{/if}
			</div>
			<div class="flex-1 min-w-0 pt-1">
				<h1 class="font-display text-2xl font-bold leading-tight">{skill.name}</h1>
				{#if skill.authorName}
					<button type="button" class="text-base text-primary font-medium mt-1 hover:underline">{skill.authorName}</button>
				{/if}
				{#if skill.category}
					<p class="text-sm text-base-content/60 mt-0.5">{skill.category}</p>
				{/if}
				<button type="button" onclick={installProduct} disabled={installing} class="h-9 px-6 rounded-full bg-primary text-primary-content font-bold text-base mt-3 hover:brightness-110 active:scale-[0.97] transition-all inline-flex items-center gap-1.5 disabled:opacity-50">
					{installing ? 'Installing...' : 'Install'}
				</button>
			</div>
		</div>
	</div>

	<!-- Stats strip -->
	<div class="px-5 py-3 border-t border-b border-base-content/5">
		<div class="flex items-center divide-x divide-base-content/10">
			<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
				{#if skill.ratingCount > 0}
					<div class="flex items-center gap-1">
						<span class="text-base font-bold text-base-content/90">{avgRating}</span>
						<Star class="w-3.5 h-3.5 text-base-content/90" />
					</div>
					<span class="text-sm text-base-content/60">{skill.ratingCount} Ratings</span>
				{:else}
					<span class="text-base font-bold text-base-content/90">--</span>
					<span class="text-sm text-base-content/60">No Ratings</span>
				{/if}
			</div>
			<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
				<span class="text-base font-bold text-base-content/90">{formatNumber(skill.installCount)}</span>
				<span class="text-sm text-base-content/60">Installs</span>
			</div>
			<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
				<span class="text-base font-bold text-success">Free</span>
				<span class="text-sm text-base-content/60">Price</span>
			</div>
			{#if skill.category}
				<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
					<span class="text-base font-bold text-base-content/80 truncate max-w-full">{skill.category}</span>
					<span class="text-sm text-base-content/60">Category</span>
				</div>
			{/if}
		</div>
	</div>

	<!-- Media Gallery -->
	<MediaGallery media={screenshots} />

	<!-- Description -->
	{#if skill.description}
		<div class="px-5 py-5 border-b border-base-content/5">
			<p class="text-base text-base-content/80 leading-relaxed">{skill.description}</p>
		</div>
	{/if}

	<!-- Ratings & Reviews -->
	<div class="px-5 py-5 border-b border-base-content/5">
		<div class="flex items-center justify-between mb-4">
			<h3 class="font-display text-lg font-bold">Ratings & Reviews</h3>
			{#if reviews.length > 0}
				<button type="button" onclick={() => showReview = true} class="text-base text-primary font-medium">See All</button>
			{/if}
		</div>

		<div class="flex gap-6">
			<!-- Big rating number -->
			<div class="shrink-0 flex flex-col items-center">
				<span class="text-5xl font-bold leading-none">{avgRating ?? '--'}</span>
				<span class="text-sm text-base-content/60 mt-1">out of 5</span>
				{#if skill.ratingCount > 0}
					<div class="flex items-center gap-0.5 mt-2">
						{#each renderStars(Math.round(skill.ratingAvg)) as filled}
							<Star class="w-3.5 h-3.5 {filled ? 'text-warning fill-warning' : 'text-base-content/40'}" />
						{/each}
					</div>
					<span class="text-sm text-base-content/60 mt-1">{skill.ratingCount} Ratings</span>
				{/if}
			</div>

			<!-- Review cards (horizontal scroll) -->
			{#if reviews.length > 0}
				<div class="flex-1 flex gap-3 overflow-x-auto pb-2 min-w-0">
					{#each reviews as review}
						<ReviewCard {review} reply={replyMap[review.id] ?? null} />
					{/each}
					<button type="button" onclick={() => showReview = true} class="shrink-0 w-48 rounded-2xl border-2 border-dashed border-base-content/10 flex flex-col items-center justify-center gap-2 hover:border-primary/30 hover:bg-primary/5 transition-colors">
						<Star class="w-6 h-6 text-base-content/40" />
						<span class="text-sm font-medium text-base-content/60">Write a Review</span>
					</button>
				</div>
			{:else}
				<div class="flex-1 flex flex-col items-center justify-center py-4">
					<p class="text-base text-base-content/80">No reviews yet</p>
					<button type="button" onclick={() => showReview = true} class="text-base text-primary font-medium mt-2">Be the first to review</button>
				</div>
			{/if}
		</div>
	</div>

	<!-- Feedback -->
	<FeedbackSection feedback={feedbackItems} targetId={skillId} targetType="skill" />

	<!-- You Might Also Like -->
	{#if similarItems.length > 0}
		<div class="px-5 py-5 border-b border-base-content/5">
			<h3 class="font-display text-lg font-bold mb-4">You Might Also Like</h3>
			<div class="flex gap-3 overflow-x-auto scrollbar-hide pb-2">
				{#each similarItems as item}
					<a href={itemHref(item)} class="flex-shrink-0 w-36 flex flex-col items-center gap-2 p-4 rounded-2xl bg-base-content/[0.03] hover:bg-base-content/[0.06] transition-colors">
						<div class="w-14 h-14 rounded-2xl {item.iconBg} flex items-center justify-center text-2xl">{item.iconEmoji}</div>
						<p class="text-sm font-semibold text-center truncate w-full">{item.name}</p>
						{#if item.code}<InstallCode code={item.code} compact />{/if}
					</a>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Information grid -->
	<div class="px-5 py-5 border-b border-base-content/5">
		<h3 class="font-display text-lg font-bold mb-4">Information</h3>
		<div class="grid grid-cols-2 gap-y-4 gap-x-8">
			{#if skill.authorName}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">Developer</p>
					<p class="text-base font-medium">{skill.authorName}</p>
				</div>
			{/if}
			{#if skill.version}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">Version</p>
					<p class="text-base font-medium">{skill.version}</p>
				</div>
			{/if}
			{#if skill.category}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">Category</p>
					<p class="text-base font-medium">{skill.category}</p>
				</div>
			{/if}
			{#if skill.type}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">Type</p>
					<p class="text-base font-medium capitalize">{skill.type}</p>
				</div>
			{/if}
			<div>
				<p class="text-sm text-base-content/60 mb-0.5">Installs</p>
				<p class="text-base font-medium">{formatNumber(skill.installCount)}</p>
			</div>
		</div>
	</div>

	<!-- Write Review Modal -->
	{#if showReview}
		<div class="fixed inset-0 z-50 flex items-center justify-center">
			<button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => showReview = false}></button>
			<div class="relative bg-base-100 rounded-2xl border border-base-content/10 w-full max-w-sm mx-4 p-6">
				<div class="flex items-center justify-between mb-4">
					<h3 class="font-display text-lg font-bold">Review {skill.name}</h3>
					<button type="button" onclick={() => showReview = false} class="p-1.5 rounded-full hover:bg-base-content/5 transition-colors">
						<X class="w-4 h-4 text-base-content/90" />
					</button>
				</div>
				<div class="space-y-4">
					<div>
						<p class="text-sm font-semibold text-base-content/60 uppercase tracking-wider mb-2">Rating</p>
						<div class="flex gap-1">
							{#each [1, 2, 3, 4, 5] as n}
								<button type="button" onclick={() => reviewRating = n}>
									<Star class="w-8 h-8 transition-colors {n <= reviewRating ? 'text-warning fill-warning' : 'text-base-content/90'}" />
								</button>
							{/each}
						</div>
					</div>
					<div>
						<label for="rev-text" class="text-sm font-semibold text-base-content/60 uppercase tracking-wider">Your Review</label>
						<textarea id="rev-text" bind:value={reviewText} rows="4" class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base placeholder:text-base-content/80 focus:outline-none focus:border-primary/50 transition-colors resize-none" placeholder="Share your experience with this skill..."></textarea>
					</div>
				</div>
				<div class="flex gap-2 mt-5">
					<button type="button" onclick={() => showReview = false} class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors">Cancel</button>
					<button type="button" disabled={!reviewText || submittingReview} onclick={submitReview} class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30">
						{submittingReview ? 'Submitting...' : 'Submit Review'}
					</button>
				</div>
			</div>
		</div>
	{/if}
{/if}
</div>
