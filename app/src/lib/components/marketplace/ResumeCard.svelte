<script lang="ts">
	import Star from 'lucide-svelte/icons/star';
	import ArrowRight from 'lucide-svelte/icons/arrow-right';
	import Check from 'lucide-svelte/icons/check';
	import { goto } from '$lib/nav';
	import { type AppItem, itemHref } from '$lib/types/marketplace';

	let {
		item,
		department = '',
		title = '',
		responsibilities = []
	}: { item: AppItem; department?: string; title?: string; responsibilities?: string[] } = $props();

	// Job title from the curated map when provided, else the artifact name.
	const jobTitle = $derived(title || item.name);

	// Initials from the job title (first two word-initials).
	const initials = $derived(
		jobTitle
			.split(/\s+/)
			.filter((w) => /^[A-Za-z]/.test(w))
			.slice(0, 2)
			.map((w) => w[0])
			.join('')
			.toUpperCase()
	);

	const href = $derived(itemHref(item));
	const hasHistory = $derived(item.installs > 0 || item.ratingCount > 0);
	const fullStars = $derived(Math.round(item.rating));
	const fmt = (n: number) => n.toLocaleString('en-US');
</script>

<article
	class="flex flex-col h-full bg-base-100 border border-base-300 rounded-2xl p-6 shadow-sm transition-all hover:-translate-y-0.5 hover:border-base-content/20 hover:shadow-md"
>
	<!-- header: face · title/dept (title never truncates) -->
	<div class="flex items-start gap-4">
		<div
			class="w-12 h-12 shrink-0 grid place-items-center rounded-full bg-primary/10 text-primary font-semibold text-base"
		>
			{initials}
		</div>
		<div class="min-w-0">
			<h3 class="text-lg font-bold tracking-tight leading-tight">{jobTitle}</h3>
			<div class="text-sm text-base-content/60 mt-0.5">
				{department || item.category}{item.author ? ` · by ${item.author}` : ''}
			</div>
		</div>
	</div>

	<!-- compensation on its own line, under the header -->
	<div class="mt-3">
		{#if item.free}
			<span class="text-base font-bold text-success tracking-tight">Free</span>
		{:else}
			<span class="text-base font-bold tracking-tight tabular-nums">{item.price}</span>
			<span class="text-xs text-base-content/60"> per seat</span>
		{/if}
	</div>

	<!-- recruiter summary -->
	{#if item.description}
		<p class="text-sm text-base-content/70 leading-relaxed mt-4 line-clamp-3">{item.description}</p>
	{/if}

	<!-- responsibilities (folded workflows) -->
	{#if responsibilities.length}
		<div class="text-[13px] font-semibold mt-5 mb-2.5">Responsibilities</div>
		<ul class="flex flex-col gap-2">
			{#each responsibilities.slice(0, 4) as duty}
				<li class="flex items-start gap-2.5 text-sm text-base-content/70 leading-snug">
					<span class="mt-0.5 w-[18px] h-[18px] shrink-0 grid place-items-center rounded-full bg-success/10 text-success"><Check class="w-3 h-3" stroke-width={2.5} /></span>
					{duty}
				</li>
			{/each}
			{#if responsibilities.length > 4}
				<li class="text-[13px] text-base-content/50 pl-[28px]">+{responsibilities.length - 4} more</li>
			{/if}
		</ul>
	{/if}

	<!-- footer: experience + action -->
	<div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-3 mt-auto pt-5">
		{#if hasHistory}
			<div class="flex flex-col gap-0.5 text-[13px] text-base-content/70">
				{#if item.ratingCount > 0}
					<span class="flex items-center gap-1.5">
						<span class="flex text-warning">
							{#each Array(5) as _, i}
								<Star class="w-3.5 h-3.5" fill={i < fullStars ? 'currentColor' : 'none'} stroke-width={1.5} />
							{/each}
						</span>
						<b class="font-semibold tabular-nums">{item.rating.toFixed(1)}</b>
						<span class="text-base-content/50">· {item.ratingCount} reviews</span>
					</span>
				{/if}
				{#if item.installs > 0}
					<span><b class="font-semibold tabular-nums">{fmt(item.installs)}</b> hires</span>
				{/if}
			</div>
		{:else}
			<span
				class="inline-flex items-center text-xs font-semibold text-primary bg-primary/10 rounded-full px-3.5 py-1.5"
				>New hire · be their first</span
			>
		{/if}
		<div class="flex gap-2.5 shrink-0">
			<button
				onclick={() => goto(href)}
				class="btn btn-primary btn-sm rounded-full gap-1.5 px-4"
				>Meet<ArrowRight class="w-4 h-4" /></button
			>
		</div>
	</div>
</article>
