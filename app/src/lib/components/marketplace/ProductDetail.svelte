<!--
  ProductDetail — the single marketplace detail page shared by every artifact
  type (skill / agent / plugin / connector / app / collection). Layout mirrors
  neboai.com's ArtifactDetail (hero + sticky install rail, long-form About,
  automations, ratings histogram, reviews, information sidebar), translated to
  Nebo's DaisyUI token system and wired to Nebo's APIs + three-state install.

  Routes under /marketplace/<plural>/[id] render this with `artifactType` set.
-->
<script lang="ts">
	import { Star, X, Check, Copy, ShieldCheck, ChevronLeft, Download } from 'lucide-svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { marked } from 'marked';
	import webapi from '$lib/api/gocliRequest';
	import { installStoreProduct, uninstallStoreProduct } from '$lib/api/nebo';
	import { type AppItem, toAppItem, itemHref, gradients } from '$lib/types/marketplace';
	import MediaGallery from '$lib/components/marketplace/MediaGallery.svelte';
	import SimilarGrid from '$lib/components/marketplace/SimilarGrid.svelte';
	import FeedbackSection from '$lib/components/marketplace/FeedbackSection.svelte';
	import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';

	type ArtifactType = 'skill' | 'agent' | 'plugin' | 'connector' | 'app' | 'collection';

	let { artifactType = 'skill' }: { artifactType?: ArtifactType } = $props();

	// Customer-facing label per type. Note "Connection" not "Connector" on the
	// buyer-facing surface — the founder's rule, mirrored from neboai.com.
	const kindConfig: Record<ArtifactType, { label: string; chip: string }> = {
		skill: { label: 'Skill', chip: 'bg-base-200 text-base-content/70' },
		agent: { label: 'Agent', chip: 'bg-accent/15 text-accent' },
		plugin: { label: 'Plugin', chip: 'bg-warning/15 text-warning' },
		connector: { label: 'Connection', chip: 'bg-success/15 text-success' },
		app: { label: 'App', chip: 'bg-accent/15 text-accent' },
		collection: { label: 'Collection', chip: 'bg-base-200 text-base-content/70' }
	};
	const kind = $derived(kindConfig[artifactType] ?? kindConfig.skill);

	// ── Loaded data ──────────────────────────────────────────────────────
	let skill: any = $state(null);
	let reviews: any[] = $state([]);
	let reviewReplies: any[] = $state([]);
	let feedbackItems: any[] = $state([]);
	let screenshots: any[] = $state([]);
	let similarItems: AppItem[] = $state([]);
	let loading = $state(true);

	// ── UI state ─────────────────────────────────────────────────────────
	let iconError = $state(false);
	let showReview = $state(false);
	let reviewRating = $state(5);
	let reviewText = $state('');
	let codeCopied = $state(false);
	let submittingReview = $state(false);
	let installing = $state(false);
	let updating = $state(false);
	let installedLocal = $state(false);
	let showSetupModal = $state(false);
	let setupInputs = $state<Record<string, unknown>>({});
	// Configure mode reuses the setup modal against the already-installed agent.
	let configureExisting = $state(false);

	const itemId = $derived($page.params.id ?? '');
	const installed = $derived(Boolean(skill?.installed) || installedLocal);

	const authorName = $derived(skill?.authorName || skill?.author?.name || '');
	const authorVerified = $derived(Boolean(skill?.authorVerified ?? skill?.author?.verified));

	const replyMap = $derived.by(() => {
		const map: Record<string, any> = {};
		for (const rr of reviewReplies) map[rr.review_id] = rr;
		return map;
	});

	// Agents can ship automations (scheduled or event-triggered workflows). The
	// names + cadence are a key selling point; the steps/logic stay private.
	const workflows = $derived.by(() => {
		const w = skill?.typeConfig?.workflows;
		if (!w || typeof w !== 'object' || Array.isArray(w)) return [] as { name: string; when: string }[];
		return Object.entries(w).map(([key, def]: [string, any]) => {
			const t = def?.trigger ?? {};
			let when = 'On demand';
			if (t.type === 'cron' || t.cron || t.schedule) when = 'On a schedule';
			else if (t.type === 'event' || Array.isArray(t.sources)) {
				const src: string = (t.sources?.[0] ?? '').toString();
				if (/email/.test(src)) when = 'When a new email arrives';
				else if (/calendar|event/.test(src)) when = 'On calendar changes';
				else when = src ? `When ${src.split('.').slice(-2).join(' ').replace(/[._]/g, ' ')}` : 'Automatic';
			}
			const name = key.replace(/[-_]/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
			return { name, when };
		});
	});
	const hasWorkflows = $derived(workflows.length > 0);

	const ratingAvgNum = $derived(skill?.ratingAvg ? Number(skill.ratingAvg) : skill?.rating ? Number(skill.rating) : 0);
	const avgRating = $derived(ratingAvgNum > 0 ? ratingAvgNum.toFixed(1) : null);
	const ratingCount = $derived(Number(skill?.ratingCount ?? skill?.reviewCount ?? 0));

	// 5-bar histogram derived from the loaded reviews.
	const ratingDistribution = $derived.by(() => {
		const buckets = [0, 0, 0, 0, 0]; // index 0 = 5 stars
		for (const r of reviews) {
			const rating = Math.max(1, Math.min(5, Math.round(r.rating ?? 0)));
			buckets[5 - rating]++;
		}
		const total = buckets.reduce((a, b) => a + b, 0) || 1;
		return buckets.map((count, i) => ({ stars: 5 - i, count, pct: Math.round((count / total) * 100) }));
	});

	// The long description is only shown when it actually adds detail beyond the
	// short tagline rendered in the hero — never echo the same sentence twice.
	const hasLongDescription = $derived(
		typeof skill?.longDescription === 'string' &&
		skill.longDescription.trim().length > 0 &&
		skill.longDescription.trim() !== (skill?.description ?? '').trim()
	);
	const longHtml = $derived(hasLongDescription ? (marked.parse(skill.longDescription, { async: false }) as string) : '');

	const information = $derived.by(() => {
		const rows: { label: string; value: string; mono?: boolean }[] = [];
		if (authorName) rows.push({ label: 'Developer', value: authorName });
		if (skill?.version) rows.push({ label: 'Version', value: `v${skill.version}` });
		if (skill?.updatedAt) {
			try {
				rows.push({ label: 'Last updated', value: new Date(skill.updatedAt).toLocaleDateString() });
			} catch { /* skip */ }
		}
		rows.push({ label: 'Installs', value: formatNumber(Number(skill?.installCount ?? 0)) });
		if (skill?.code) rows.push({ label: 'Install code', value: skill.code, mono: true });
		return rows;
	});

	const iconIsUrl = $derived(skill?.icon && (String(skill.icon).startsWith('http') || String(skill.icon).startsWith('/')));
	const iconGradient = $derived.by(() => {
		const s = skill?.slug || skill?.name || '';
		let hash = 0;
		for (let i = 0; i < s.length; i++) hash = ((hash << 5) - hash + s.charCodeAt(i)) | 0;
		return gradients[Math.abs(hash) % gradients.length];
	});

	onMount(async () => {
		try {
			const [skillRes, reviewsRes, mediaRes, feedbackRes] = await Promise.all([
				webapi.get<any>(`/api/v1/store/products/${itemId}`),
				webapi.get<any>(`/api/v1/store/products/${itemId}/reviews`).catch(() => ({ reviews: [] })),
				webapi.get<any>(`/api/v1/store/products/${itemId}/media`).catch(() => ({ media: [] })),
				webapi.get<any>(`/api/v1/store/products/${itemId}/feedback`).catch(() => ({ feedback: [] }))
			]);
			skill = skillRes?.id ? skillRes : null;
			reviews = reviewsRes.reviews ?? [];
			screenshots = mediaRes.media ?? [];
			feedbackItems = feedbackRes.feedback ?? [];
		} catch {
			skill = null;
		}
		loading = false;
		webapi
			.get<any>(`/api/v1/store/products/${itemId}/similar`)
			.then((res: any) => {
				similarItems = (res.products || []).map((a: any, i: number) => toAppItem(a, i));
			})
			.catch(() => {});
	});

	async function installProduct() {
		// Agents always go through the setup modal — it handles inputs, plugin
		// auth, dependency install, scheduling, and activation (and shows a
		// "ready to go" step when there's nothing to configure). Non-agents
		// install directly.
		if (artifactType === 'agent') {
			setupInputs = skill?.typeConfig?.inputs || skill?.inputs || {};
			configureExisting = false;
			showSetupModal = true;
			return;
		}
		installing = true;
		try {
			await installStoreProduct(itemId);
			installedLocal = true;
		} catch { /* ignore */ }
		installing = false;
	}

	// Re-open the setup modal against the installed agent (the installed agent id
	// equals the product id), so users can edit inputs / reschedule / uninstall.
	function configureAgent() {
		setupInputs = skill?.typeConfig?.inputs || skill?.inputs || {};
		configureExisting = true;
		showSetupModal = true;
	}

	async function uninstallProduct() {
		try {
			await uninstallStoreProduct(itemId);
		} catch { /* ignore */ }
		installedLocal = false;
		if (skill) skill = { ...skill, installed: false };
		showSetupModal = false;
		configureExisting = false;
	}

	async function updateProduct() {
		updating = true;
		try {
			await webapi.post(`/api/v1/artifacts/${itemId}/apply-update`, {});
			const skillRes = await webapi.get<any>(`/api/v1/store/products/${itemId}`);
			skill = skillRes?.id ? skillRes : skill;
		} catch { /* ignore */ }
		updating = false;
	}

	async function submitReview() {
		submittingReview = true;
		try {
			await webapi.post(`/api/v1/store/products/${itemId}/reviews`, { rating: reviewRating, body: reviewText });
			showReview = false;
			reviewText = '';
			const res = await webapi.get<any>(`/api/v1/store/products/${itemId}/reviews`);
			reviews = res.reviews ?? [];
		} catch { /* ignore */ }
		submittingReview = false;
	}

	async function copyCode() {
		if (!skill?.code) return;
		await navigator.clipboard.writeText(skill.code);
		codeCopied = true;
		setTimeout(() => (codeCopied = false), 2000);
	}

	function handleSetupComplete(newAgentId: string) {
		showSetupModal = false;
		goto(`/${newAgentId}/threads`);
	}

	function formatNumber(n: number) {
		if (!n) return '0';
		if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
		return n.toString();
	}

	function renderStars(rating: number) {
		return Array.from({ length: 5 }, (_, i) => i < rating);
	}

	function initials(name: string) {
		return (name || '')
			.split(' ')
			.map((w: string) => w[0])
			.join('')
			.slice(0, 2)
			.toUpperCase();
	}
</script>

<div class="max-w-6xl mx-auto px-4 sm:px-6 lg:px-8 pt-8 pb-16">
	{#if loading}
		<div class="flex justify-center py-24">
			<span class="loading loading-spinner loading-md text-primary"></span>
		</div>
	{:else if !skill}
		<div class="flex flex-col items-center justify-center py-24 text-center">
			<p class="text-sm text-base-content/50">{kind.label} not found</p>
			<a href="/marketplace" class="text-sm text-primary font-medium mt-3 hover:underline">Browse marketplace</a>
		</div>
	{:else}
		<!-- Back link -->
		<a href="/marketplace" class="inline-flex items-center gap-1 text-sm text-base-content/50 hover:text-base-content transition-colors" aria-label="Back to marketplace">
			<ChevronLeft class="w-4 h-4" />
			<span>Marketplace</span>
		</a>

		<!-- Hero: title block + screenshots / sticky install rail -->
		<header class="grid lg:grid-cols-[1fr_360px] gap-8 lg:gap-12 items-start mt-2 mb-10">
			<div class="min-w-0 flex flex-col gap-8">
				<div class="flex items-start gap-4 min-w-0">
					<div class="w-20 h-20 rounded-2xl {iconIsUrl ? 'bg-base-200 border border-base-300' : iconGradient} grid place-items-center shrink-0 overflow-hidden shadow-lg">
						{#if iconIsUrl && !iconError}
							<img src={skill.icon} alt="" class="w-20 h-20 object-cover" onerror={() => (iconError = true)} />
						{:else if skill.icon && !iconIsUrl}
							<span class="text-4xl">{skill.icon}</span>
						{:else}
							<span class="text-3xl font-bold text-white drop-shadow-md">{initials(skill.name)}</span>
						{/if}
					</div>
					<div class="min-w-0 flex-1">
						<div class="flex items-center gap-2 flex-wrap mb-2">
							<span class="text-xs font-medium px-2.5 py-1 rounded-full {kind.chip}">{kind.label}</span>
							{#if authorVerified}
								<span class="text-xs font-medium px-2.5 py-1 rounded-full bg-success/15 text-success inline-flex items-center gap-1">
									<ShieldCheck class="w-3 h-3" />
									Signed
								</span>
							{/if}
							{#if skill.category}
								<span class="text-xs font-medium px-2.5 py-1 rounded-full bg-base-200 text-base-content/70">{skill.category}</span>
							{/if}
						</div>
						<h1 class="font-display text-3xl sm:text-4xl font-bold tracking-tight leading-[1.1]">{skill.name}</h1>
						{#if skill.description}
							<p class="text-base sm:text-lg text-base-content/70 mt-3 max-w-2xl leading-snug">{skill.description}</p>
						{/if}
						<div class="flex items-center gap-4 text-sm text-base-content/50 mt-4 flex-wrap">
							{#if authorName}
								<span>by <span class="text-base-content/80 font-medium">{authorName}</span></span>
							{/if}
							{#if avgRating}
								<span class="inline-flex items-center gap-1">
									<Star class="w-3.5 h-3.5 text-warning fill-warning" />
									<span class="text-base-content/80 font-medium">{avgRating}</span>
									<span class="text-base-content/50">({ratingCount})</span>
								</span>
							{/if}
							<span>{formatNumber(Number(skill.installCount ?? 0))} installs</span>
						</div>
					</div>
				</div>

				<!-- Screenshots -->
				{#if screenshots.length > 0}
					<MediaGallery media={screenshots} />
				{/if}
			</div>

			<!-- Right: sticky install rail -->
			<aside class="lg:sticky lg:top-6">
				<div class="rounded-2xl bg-base-100 border border-base-300 shadow-lg p-5 flex flex-col gap-4">
					<div>
						<div class="text-xs uppercase tracking-wider font-semibold text-base-content/50">Price</div>
						<div class="font-display text-2xl font-bold mt-0.5">{skill.price && skill.price !== 'Get' ? skill.price : 'Free'}</div>
					</div>

					{#if skill.code}
						<div class="flex flex-col gap-1.5">
							<span class="text-xs text-base-content/50 font-medium">Install code</span>
							<button type="button" onclick={copyCode} class="flex items-center justify-between gap-3 p-3 rounded-xl bg-base-200 border border-base-300 hover:bg-base-300 transition-colors group">
								<span class="font-mono text-sm font-bold tracking-wider">{skill.code}</span>
								{#if codeCopied}
									<Check class="w-4 h-4 text-success shrink-0" />
								{:else}
									<Copy class="w-4 h-4 text-base-content/50 group-hover:text-base-content shrink-0 transition-colors" />
								{/if}
							</button>
							<p class="text-xs text-base-content/50 leading-relaxed">Paste into Nebo's chat to install on any companion.</p>
						</div>
					{/if}

					{#if installed && skill.updateAvailable}
						<button type="button" onclick={updateProduct} disabled={updating} class="btn btn-accent rounded-xl h-11 disabled:opacity-50">
							{updating ? 'Updating…' : `Update to v${skill.remoteVersion}`}
						</button>
					{:else if installed}
						<span class="h-11 rounded-xl bg-success/15 text-success font-bold inline-flex items-center justify-center gap-1.5">
							<Check class="w-4 h-4" />
							Installed
						</span>
						{#if artifactType === 'agent'}
							<button type="button" onclick={configureAgent} class="btn btn-outline rounded-xl h-11">
								Configure
							</button>
						{/if}
					{:else}
						<button type="button" onclick={installProduct} disabled={installing} class="btn btn-primary rounded-xl h-11 disabled:opacity-50">
							<Download class="w-4 h-4" />
							{installing ? 'Installing…' : 'Install'}
						</button>
					{/if}

					{#if avgRating}
						<div class="pt-3 border-t border-base-300 flex items-center justify-between text-sm">
							<span class="inline-flex items-center gap-1 text-base-content/70">
								<Star class="w-3.5 h-3.5 text-warning fill-warning" />
								<span class="font-medium text-base-content">{avgRating}</span>
							</span>
							<span class="text-xs text-base-content/50">{ratingCount} reviews</span>
						</div>
					{/if}
				</div>
			</aside>
		</header>

		<!-- Body: long content + meta sidebar -->
		<div class="grid lg:grid-cols-[1fr_320px] gap-10 lg:gap-14">
			<div class="min-w-0 flex flex-col gap-12">
				<!-- About — long description only; the short tagline lives in the hero -->
				{#if hasLongDescription}
					<section class="flex flex-col gap-3">
						<h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">About this {kind.label.toLowerCase()}</h2>
						<div class="prose prose-sm sm:prose-base max-w-none prose-headings:font-display prose-headings:tracking-tight">
							{@html longHtml}
						</div>
					</section>
				{/if}

				<!-- Automations -->
				{#if hasWorkflows}
					<section class="flex flex-col gap-3">
						<h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">What it runs for you</h2>
						<p class="text-sm text-base-content/50 max-w-2xl">Automations that run on a schedule or when something happens, so you don't have to lift a finger.</p>
						<div class="flex flex-col gap-2">
							{#each workflows as wf}
								<div class="flex items-center justify-between gap-3 px-4 py-3 rounded-xl bg-base-100 border border-base-300">
									<span class="text-sm font-medium">{wf.name}</span>
									<span class="text-xs font-medium px-2.5 py-1 rounded-full bg-base-200 text-base-content/70 shrink-0">{wf.when}</span>
								</div>
							{/each}
						</div>
					</section>
				{/if}

				<!-- Ratings + histogram -->
				<section class="flex flex-col gap-3">
					<h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Ratings &amp; reviews</h2>
					<div class="rounded-2xl bg-base-100 border border-base-300 p-5">
						{#if avgRating}
							<div class="grid grid-cols-1 sm:grid-cols-[auto_1fr] gap-6 items-center">
								<div class="flex sm:flex-col items-center sm:items-start gap-3 sm:gap-1">
									<span class="font-display text-5xl font-bold leading-none">{avgRating}</span>
									<div class="flex flex-col gap-0.5">
										<div class="flex items-center gap-0.5">
											{#each renderStars(Math.round(ratingAvgNum)) as filled}
												<Star class="w-4 h-4 {filled ? 'text-warning fill-warning' : 'text-base-content/30'}" />
											{/each}
										</div>
										<span class="text-xs text-base-content/50">{ratingCount} review{ratingCount === 1 ? '' : 's'}</span>
									</div>
								</div>
								<div class="flex flex-col gap-1.5">
									{#each ratingDistribution as bar}
										<div class="flex items-center gap-2 text-xs">
											<span class="w-6 text-base-content/50 tabular-nums text-right">{bar.stars}</span>
											<Star class="w-3 h-3 text-warning fill-warning shrink-0" />
											<div class="flex-1 h-2 rounded-full bg-base-300 overflow-hidden">
												<div class="h-full bg-warning rounded-full" style="width: {bar.pct}%"></div>
											</div>
											<span class="w-8 text-base-content/50 tabular-nums text-right">{bar.count}</span>
										</div>
									{/each}
								</div>
							</div>
						{:else}
							<p class="text-sm text-base-content/50">No ratings yet. Be the first to leave one.</p>
						{/if}
					</div>

					{#if reviews.length > 0}
						<div class="flex flex-col gap-3">
							{#each reviews.slice(0, 5) as review}
								<div class="rounded-2xl bg-base-100 border border-base-300 p-5 flex flex-col gap-2">
									<div class="flex items-center justify-between gap-3">
										<div class="text-sm font-semibold truncate">{review.reviewerName || review.userName || review.reviewerSlug || 'Anonymous'}</div>
										<div class="flex items-center gap-0.5">
											{#each renderStars(review.rating) as filled}
												<Star class="w-3 h-3 {filled ? 'text-warning fill-warning' : 'text-base-content/30'}" />
											{/each}
										</div>
									</div>
									<p class="text-sm text-base-content/70 leading-snug">{review.body ?? review.text ?? ''}</p>
									{#if replyMap[review.id]}
										<div class="mt-1 pt-3 border-t border-base-300">
											<p class="text-xs font-semibold text-base-content/50 mb-1">Developer response</p>
											<p class="text-xs text-base-content/70 leading-relaxed">{replyMap[review.id].body}</p>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
					<button type="button" onclick={() => (showReview = true)} class="btn btn-sm btn-ghost self-start gap-1.5">
						<Star class="w-3.5 h-3.5" />
						Write a review
					</button>
				</section>

				<!-- Feedback -->
				<FeedbackSection feedback={feedbackItems} targetId={itemId} targetType={artifactType} />
			</div>

			<!-- Right meta sidebar — Information block -->
			<aside class="flex flex-col gap-4">
				<div class="rounded-2xl bg-base-100 border border-base-300 p-5">
					<h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">Information</h3>
					<dl class="flex flex-col">
						{#each information as row}
							<div class="flex items-center justify-between gap-3 py-2 border-b border-base-content/10 last:border-b-0 text-sm">
								<dt class="text-base-content/50">{row.label}</dt>
								<dd class="font-medium text-right {row.mono ? 'font-mono' : ''}">{row.value}</dd>
							</div>
						{/each}
					</dl>
				</div>

				{#if authorName}
					<a href="/marketplace?publisher={encodeURIComponent(authorName)}" class="text-sm text-primary hover:underline text-center">
						See more by {authorName}
					</a>
				{/if}
			</aside>
		</div>

		<!-- You might also like — full width below the two-column body -->
		{#if similarItems.length > 0}
			<section class="mt-16 flex flex-col gap-4">
				<h2 class="font-display text-xl font-bold tracking-tight">You might also like</h2>
				<SimilarGrid items={similarItems.slice(0, 6)} />
			</section>
		{/if}
	{/if}
</div>

<!-- Write Review Modal -->
{#if showReview && skill}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => (showReview = false)} aria-label="Close review modal"></button>
		<div class="relative bg-base-100 rounded-2xl border border-base-300 w-full max-w-sm mx-4 p-6">
			<div class="flex items-center justify-between mb-4">
				<h3 class="font-display text-lg font-bold">Review {skill.name}</h3>
				<button type="button" onclick={() => (showReview = false)} class="p-1.5 rounded-full hover:bg-base-200 transition-colors">
					<X class="w-4 h-4" />
				</button>
			</div>
			<div class="flex flex-col gap-4">
				<div>
					<p class="text-xs font-semibold text-base-content/50 uppercase tracking-wider mb-2">Rating</p>
					<div class="flex gap-1">
						{#each [1, 2, 3, 4, 5] as n}
							<button type="button" onclick={() => (reviewRating = n)} aria-label="{n} stars">
								<Star class="w-8 h-8 transition-colors {n <= reviewRating ? 'text-warning fill-warning' : 'text-base-content/30'}" />
							</button>
						{/each}
					</div>
				</div>
				<div>
					<label for="rev-text" class="text-xs font-semibold text-base-content/50 uppercase tracking-wider">Your review</label>
					<textarea id="rev-text" bind:value={reviewText} rows="4" class="textarea textarea-bordered w-full mt-2 resize-none" placeholder="Share your experience…"></textarea>
				</div>
			</div>
			<div class="flex gap-2 mt-5">
				<button type="button" onclick={() => (showReview = false)} class="btn btn-ghost flex-1">Cancel</button>
				<button type="button" disabled={!reviewText || submittingReview} onclick={submitReview} class="btn btn-primary flex-1 disabled:opacity-40">
					{submittingReview ? 'Submitting…' : 'Submit review'}
				</button>
			</div>
		</div>
	</div>
{/if}

{#if showSetupModal && skill}
	<AgentSetupModal
		appId={itemId}
		agentName={skill.name}
		agentDescription={skill.description || ''}
		inputs={setupInputs}
		dependencies={skill?.dependencies ?? skill?.typeConfig?.dependencies}
		existingAgentId={configureExisting ? itemId : undefined}
		onComplete={handleSetupComplete}
		onCancel={() => (showSetupModal = false)}
		onUninstall={configureExisting ? uninstallProduct : undefined}
	/>
{/if}
