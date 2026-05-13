<script lang="ts">
	import { ArrowLeft, Star, X, Check, Copy } from 'lucide-svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import webapi from '$lib/api/gocliRequest';
	import { installStoreProduct, listAgents, activateAgent } from '$lib/api/nebo';
	import MediaGallery from '$lib/components/marketplace/MediaGallery.svelte';
	import ReviewCard from '$lib/components/marketplace/ReviewCard.svelte';
	import FeedbackSection from '$lib/components/marketplace/FeedbackSection.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';

	let showReview = $state(false);
	let reviewRating = $state(5);
	let reviewText = $state('');
	let codeCopied = $state(false);

	import { type AppItem, toAppItem, itemHref, gradients } from '$lib/types/marketplace';
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
	let showSetupModal = $state(false);
	let setupInputs = $state<Record<string, unknown>>({});

	const agentId = $derived($page.params.id);

	const replyMap = $derived<Record<string, any>>(() => {
		const map: Record<string, any> = {};
		for (const rr of reviewReplies) {
			map[rr.review_id] = rr;
		}
		return map;
	});

	// Parse capabilities from API response (safe summary, no IP exposed)
	interface AutomationSummary {
		name: string;
		description: string;
		triggerType: string;
		stepCount: number;
	}

	const agentIncludes = $derived(() => {
		const empty = { automations: [] as AutomationSummary[], skills: [] as string[], triggerTypes: [] as string[], hasPersona: false };
		const caps = skill?.capabilities;
		if (!caps) return empty;

		return {
			automations: caps.automations || [],
			skills: caps.skillDependencies || [],
			triggerTypes: caps.triggerTypes || [],
			hasPersona: caps.hasPersona || false
		};
	});

	onMount(async () => {
		try {
			const [skillRes, reviewsRes, mediaRes, feedbackRes] = await Promise.all([
				webapi.get<any>(`/api/v1/store/products/${agentId}`),
				webapi.get<any>(`/api/v1/store/products/${agentId}/reviews`).catch(() => ({ reviews: [] })),
				webapi.get<any>(`/api/v1/store/products/${agentId}/media`).catch(() => ({ media: [] })),
				webapi.get<any>(`/api/v1/store/products/${agentId}/feedback`).catch(() => ({ feedback: [] }))
			]);
			skill = skillRes;
			reviews = reviewsRes.reviews ?? [];
			screenshots = mediaRes.media ?? [];
			feedbackItems = feedbackRes.feedback ?? [];
		} catch {
			skill = null;
		}
		loading = false;
		webapi.get<any>(`/api/v1/store/products/${agentId}/similar`).then((res: any) => {
			similarItems = (res.apps || []).map((a: any, i: number) => toAppItem(a, i));
		}).catch(() => {});
	});

	async function submitReview() {
		submittingReview = true;
		try {
			await webapi.post(`/api/v1/store/products/${agentId}/reviews`, { rating: reviewRating, body: reviewText });
			showReview = false;
			reviewText = '';
			const res = await webapi.get<any>(`/api/v1/store/products/${agentId}/reviews`);
			reviews = res.reviews ?? [];
		} catch { /* ignore */ }
		submittingReview = false;
	}

	async function installProduct() {
		installing = true;
		try {
			// Check for inputs in agent config
			const inputs = skill?.typeConfig?.inputs || skill?.inputs || {};
			if (Object.keys(inputs).length > 0) {
				setupInputs = inputs;
				showSetupModal = true;
				installing = false;
				return;
			}

			// No inputs — install directly
			await installStoreProduct(agentId);

			// Find and activate the agent
			const agentsRes = await listAgents();
			const allAgents = agentsRes?.agents || [];
			const matched = allAgents.find(
				(r: any) => r.name?.toLowerCase() === skill?.name?.toLowerCase()
			);

			if (matched) {
				await activateAgent(matched.id);
				goto(`/agent/persona/${matched.id}/chat`);
				return;
			}
		} catch { /* ignore */ }
		installing = false;
	}

	function handleSetupComplete(newAgentId: string) {
		showSetupModal = false;
		goto(`/agent/persona/${newAgentId}/chat`);
	}

	function handleSetupCancel() {
		showSetupModal = false;
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

	// Deterministic gradient from slug so the icon color is consistent across pages
	const iconGradient = $derived(() => {
		const s = skill?.slug || skill?.name || '';
		let hash = 0;
		for (let i = 0; i < s.length; i++) hash = ((hash << 5) - hash + s.charCodeAt(i)) | 0;
		return gradients[Math.abs(hash) % gradients.length];
	});

	const iconIsUrl = $derived(skill?.icon && (skill.icon.startsWith('http') || skill.icon.startsWith('/')));
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
		<p class="text-base text-base-content/80">{$t('marketplace.detail.agentNotFound')}</p>
	</div>
{:else}
	<!-- Hero: icon + name + install button -->
	<div class="px-5 pt-6 pb-5">
		<div class="flex items-start gap-5">
			<div class="w-28 h-28 rounded-[22px] {iconIsUrl ? 'bg-gradient-to-br from-base-content/5 to-base-content/10' : iconGradient()} flex items-center justify-center shrink-0 shadow-lg">
				{#if iconIsUrl}
					<img src={skill.icon} alt="" class="w-28 h-28 rounded-[22px]" />
				{:else}
					<span class="text-4xl font-bold text-white drop-shadow-md">{(skill.name || '').split(' ').map((w: string) => w[0]).join('').slice(0, 2).toUpperCase()}</span>
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
				{#if skill.installed}
					<div class="flex items-center gap-2 mt-3">
						<span class="h-9 px-6 rounded-full bg-success/15 text-success font-bold text-base inline-flex items-center gap-1.5">
							{$t('common.installed')}
						</span>
						<button type="button" onclick={() => { setupInputs = skill?.typeConfig?.inputs || []; showSetupModal = true; }} class="h-9 px-5 rounded-full border border-base-content/15 text-base font-medium hover:bg-base-content/5 transition-colors inline-flex items-center">
							{$t('marketplace.detail.configure')}
						</button>
					</div>
				{:else}
					<button type="button" onclick={installProduct} disabled={installing} class="h-9 px-6 rounded-full bg-primary text-primary-content font-bold text-base mt-3 hover:brightness-110 active:scale-[0.97] transition-all inline-flex items-center gap-1.5 disabled:opacity-50">
						{installing ? $t('marketplace.detail.installing') : $t('common.install')}
					</button>
				{/if}
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
					<span class="text-sm text-base-content/60">{$t('marketplace.detail.ratings', { values: { count: skill.ratingCount } })}</span>
				{:else}
					<span class="text-base font-bold text-base-content/90">--</span>
					<span class="text-sm text-base-content/60">{$t('marketplace.detail.noRatings')}</span>
				{/if}
			</div>
			<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
				<span class="text-base font-bold text-base-content/90">{formatNumber(skill.installCount)}</span>
				<span class="text-sm text-base-content/60">{$t('marketplace.detail.installs')}</span>
			</div>
			{#if skill.version}
				<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
					<span class="text-base font-bold text-base-content/90">{skill.version}</span>
					<span class="text-sm text-base-content/60">{$t('marketplace.detail.version')}</span>
				</div>
			{/if}
			{#if skill.category}
				<div class="flex-1 flex flex-col items-center gap-0.5 py-1">
					<span class="text-base font-bold text-base-content/80 truncate max-w-full">{skill.category}</span>
					<span class="text-sm text-base-content/60">{$t('marketplace.detail.category')}</span>
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

	<!-- What's Included -->
	{#if agentIncludes().automations.length > 0 || agentIncludes().skills.length > 0 || agentIncludes().hasPersona}
		<div class="px-5 py-5 border-b border-base-content/5">
			<h3 class="font-display text-lg font-bold mb-4">{$t('marketplace.detail.whatYouGet')}</h3>
			<div class="space-y-5">
				{#if agentIncludes().hasPersona}
					<div class="flex items-center gap-2">
						<span class="text-sm font-medium px-3 py-1.5 rounded-full bg-primary/10 text-primary">{$t('marketplace.detail.customPersona')}</span>
					</div>
				{/if}
				{#if agentIncludes().automations.length > 0}
					<div>
						<p class="text-sm text-base-content/60 mb-2 font-medium uppercase tracking-wider">{agentIncludes().automations.length > 1 ? $t('marketplace.detail.automationCount', { values: { count: agentIncludes().automations.length } }) : $t('marketplace.detail.automationSingular', { values: { count: agentIncludes().automations.length } })}</p>
						<div class="space-y-2">
							{#each agentIncludes().automations as auto}
								<div class="rounded-xl bg-base-content/[0.03] border border-base-content/5 p-3">
									<div class="flex items-center justify-between">
										<p class="text-base font-semibold capitalize">{auto.name}</p>
										<span class="text-xs font-medium px-2 py-0.5 rounded-full bg-base-content/10 text-base-content/70">{auto.triggerType}</span>
									</div>
									{#if auto.description}
										<p class="text-sm text-base-content/70 mt-1">{auto.description}</p>
									{/if}
									{#if auto.stepCount > 0}
										<p class="text-xs text-base-content/50 mt-2">{$t('marketplace.detail.stepCount', { values: { count: auto.stepCount } })}</p>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}
				{#if agentIncludes().skills.length > 0}
					<div>
						<p class="text-sm text-base-content/60 mb-2 font-medium uppercase tracking-wider">{$t('marketplace.detail.requiredSkills')}</p>
						<div class="flex flex-wrap gap-2">
							{#each agentIncludes().skills as sk}
								<span class="text-sm font-medium px-3 py-1.5 rounded-full bg-info/10 text-info">{sk}</span>
							{/each}
						</div>
					</div>
				{/if}
			</div>
		</div>
	{/if}

	<!-- Ratings & Reviews -->
	<div class="px-5 py-5 border-b border-base-content/5">
		<div class="flex items-center justify-between mb-4">
			<h3 class="font-display text-lg font-bold">{$t('marketplace.detail.ratingsAndReviews')}</h3>
			{#if reviews.length > 0}
				<button type="button" onclick={() => showReview = true} class="text-base text-primary font-medium">{$t('marketplace.detail.seeAll')}</button>
			{/if}
		</div>

		<div class="flex gap-6">
			<!-- Big rating number -->
			<div class="shrink-0 flex flex-col items-center">
				<span class="text-5xl font-bold leading-none">{avgRating ?? '--'}</span>
				<span class="text-sm text-base-content/60 mt-1">{$t('marketplace.detail.outOf5')}</span>
				{#if skill.ratingCount > 0}
					<div class="flex items-center gap-0.5 mt-2">
						{#each renderStars(Math.round(skill.ratingAvg)) as filled}
							<Star class="w-3.5 h-3.5 {filled ? 'text-warning fill-warning' : 'text-base-content/40'}" />
						{/each}
					</div>
					<span class="text-sm text-base-content/60 mt-1">{$t('marketplace.detail.ratings', { values: { count: skill.ratingCount } })}</span>
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
						<span class="text-sm font-medium text-base-content/60">{$t('marketplace.detail.writeReview')}</span>
					</button>
				</div>
			{:else}
				<div class="flex-1 flex flex-col items-center justify-center py-4">
					<p class="text-base text-base-content/80">{$t('marketplace.detail.noReviews')}</p>
					<button type="button" onclick={() => showReview = true} class="text-base text-primary font-medium mt-2">{$t('marketplace.detail.beFirst')}</button>
				</div>
			{/if}
		</div>
	</div>

	<!-- Feedback -->
	<FeedbackSection feedback={feedbackItems} targetId={agentId} targetType="agent" />

	<!-- You Might Also Like -->
	{#if similarItems.length > 0}
		<div class="px-5 py-5 border-b border-base-content/5">
			<h3 class="font-display text-lg font-bold mb-4">{$t('marketplace.detail.youMightLike')}</h3>
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
		<h3 class="font-display text-lg font-bold mb-4">{$t('marketplace.detail.information')}</h3>
		<div class="grid grid-cols-2 gap-y-4 gap-x-8">
			{#if skill.authorName}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">{$t('marketplace.detail.developer')}</p>
					<p class="text-base font-medium">{skill.authorName}</p>
				</div>
			{/if}
			{#if skill.version}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">{$t('marketplace.detail.version')}</p>
					<p class="text-base font-medium">{skill.version}</p>
				</div>
			{/if}
			{#if skill.category}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">{$t('marketplace.detail.category')}</p>
					<p class="text-base font-medium">{skill.category}</p>
				</div>
			{/if}
			{#if skill.code}
				<div>
					<p class="text-sm text-base-content/60 mb-0.5">{$t('marketplace.detail.code')}</p>
					<p class="text-base font-medium font-mono">{skill.code}</p>
				</div>
			{/if}
		</div>
	</div>

	<!-- Write Review Modal -->
	{#if showReview}
		<div class="fixed inset-0 z-50 flex items-center justify-center">
			<button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => showReview = false}></button>
			<div class="relative bg-base-100 rounded-2xl border border-base-content/10 w-full max-w-sm mx-4 p-6">
				<div class="flex items-center justify-between mb-4">
					<h3 class="font-display text-lg font-bold">{$t('marketplace.detail.reviewTitle', { values: { name: skill.name } })}</h3>
					<button type="button" onclick={() => showReview = false} class="p-1.5 rounded-full hover:bg-base-content/5 transition-colors">
						<X class="w-4 h-4 text-base-content/90" />
					</button>
				</div>
				<div class="space-y-4">
					<div>
						<p class="text-sm font-semibold text-base-content/60 uppercase tracking-wider mb-2">{$t('marketplace.detail.rating')}</p>
						<div class="flex gap-1">
							{#each [1, 2, 3, 4, 5] as n}
								<button type="button" onclick={() => reviewRating = n}>
									<Star class="w-8 h-8 transition-colors {n <= reviewRating ? 'text-warning fill-warning' : 'text-base-content/90'}" />
								</button>
							{/each}
						</div>
					</div>
					<div>
						<label for="rev-text" class="text-sm font-semibold text-base-content/60 uppercase tracking-wider">{$t('marketplace.detail.yourReview')}</label>
						<textarea id="rev-text" bind:value={reviewText} rows="4" class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base placeholder:text-base-content/80 focus:outline-none focus:border-primary/50 transition-colors resize-none" placeholder={$t('marketplace.detail.reviewPlaceholderAgent')}></textarea>
					</div>
				</div>
				<div class="flex gap-2 mt-5">
					<button type="button" onclick={() => showReview = false} class="flex-1 h-11 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors">{$t('common.cancel')}</button>
					<button type="button" disabled={!reviewText || submittingReview} onclick={submitReview} class="flex-1 h-11 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30">
						{submittingReview ? $t('marketplace.detail.submitting') : $t('marketplace.detail.submitReview')}
					</button>
				</div>
			</div>
		</div>
	{/if}
{/if}
</div>

{#if showSetupModal && skill}
	<AgentSetupModal
		appId={agentId}
		agentName={skill.name}
		agentDescription={skill.description || ''}
		inputs={setupInputs}
		onComplete={handleSetupComplete}
		onCancel={handleSetupCancel}
	/>
{/if}
