<!--
  Carousel/Gallery Component
  Responsive image carousel with navigation and indicators
-->

<script lang="ts">
	import { ChevronLeft, ChevronRight } from 'lucide-svelte';

	interface Slide {
		src: string;
		alt?: string;
		title?: string;
		description?: string;
	}

	interface Props {
		slides?: Slide[];
		autoplay?: boolean;
		interval?: number;
		showArrows?: boolean;
		showIndicators?: boolean;
		aspectRatio?: 'auto' | 'square' | 'video' | 'wide';
		loop?: boolean;
		class?: string;
		onchange?: (index: number) => void;
		children?: any;
	}

	let {
		slides = [],
		autoplay = false,
		interval = 5000,
		showArrows = true,
		showIndicators = true,
		aspectRatio = 'video',
		loop = true,
		class: extraClass = '',
		onchange,
		children
	}: Props = $props();

	let currentIndex = $state(0);
	let autoplayTimer: ReturnType<typeof setInterval> | null = null;
	let isHovering = $state(false);

	const totalSlides = $derived(slides.length);

	const aspectClasses: Record<string, string> = {
		auto: '',
		square: 'aspect-square',
		video: 'aspect-video',
		wide: 'aspect-[21/9]'
	};

	function goTo(index: number) {
		if (index < 0) {
			currentIndex = loop ? totalSlides - 1 : 0;
		} else if (index >= totalSlides) {
			currentIndex = loop ? 0 : totalSlides - 1;
		} else {
			currentIndex = index;
		}
		onchange?.(currentIndex);
	}

	function next() {
		goTo(currentIndex + 1);
	}

	function prev() {
		goTo(currentIndex - 1);
	}

	function startAutoplay() {
		if (autoplay && totalSlides > 1 && !autoplayTimer) {
			autoplayTimer = setInterval(next, interval);
		}
	}

	function stopAutoplay() {
		if (autoplayTimer) {
			clearInterval(autoplayTimer);
			autoplayTimer = null;
		}
	}

	// Handle autoplay
	$effect(() => {
		if (autoplay && !isHovering) {
			startAutoplay();
		} else {
			stopAutoplay();
		}
		return () => stopAutoplay();
	});

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowLeft') {
			prev();
		} else if (e.key === 'ArrowRight') {
			next();
		}
	}
</script>

<div
	class="relative w-full overflow-hidden rounded-lg bg-base-200 {extraClass}"
	role="region"
	aria-roledescription="carousel"
	aria-label="Image carousel"
	onmouseenter={() => (isHovering = true)}
	onmouseleave={() => (isHovering = false)}
	onkeydown={handleKeydown}
	tabindex="0"
>
	<!-- Slides Container -->
	<div class="relative {aspectClasses[aspectRatio]}">
		{#if children}
			{@render children()}
		{:else}
			{#each slides as slide, index}
				<div
					class="absolute inset-0 transition-opacity duration-500"
					class:opacity-100={index === currentIndex}
					class:opacity-0={index !== currentIndex}
					class:pointer-events-none={index !== currentIndex}
					role="group"
					aria-roledescription="slide"
					aria-label="Slide {index + 1} of {totalSlides}"
					aria-hidden={index !== currentIndex}
				>
					<img
						src={slide.src}
						alt={slide.alt || slide.title || `Slide ${index + 1}`}
						class="w-full h-full object-cover"
					/>
					{#if slide.title || slide.description}
						<div class="absolute bottom-0 left-0 right-0 p-4 bg-gradient-to-t from-black/60 to-transparent">
							{#if slide.title}
								<h3 class="text-white font-semibold text-lg">{slide.title}</h3>
							{/if}
							{#if slide.description}
								<p class="text-white/80 text-sm mt-1">{slide.description}</p>
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		{/if}
	</div>

	<!-- Navigation Arrows -->
	{#if showArrows && totalSlides > 1}
		<button
			type="button"
			class="absolute left-2 top-1/2 -translate-y-1/2 btn btn-circle btn-sm bg-base-100/80 hover:bg-base-100 border-0 shadow-lg"
			onclick={prev}
			aria-label="Previous slide"
			disabled={!loop && currentIndex === 0}
		>
			<ChevronLeft class="w-5 h-5" />
		</button>
		<button
			type="button"
			class="absolute right-2 top-1/2 -translate-y-1/2 btn btn-circle btn-sm bg-base-100/80 hover:bg-base-100 border-0 shadow-lg"
			onclick={next}
			aria-label="Next slide"
			disabled={!loop && currentIndex === totalSlides - 1}
		>
			<ChevronRight class="w-5 h-5" />
		</button>
	{/if}

	<!-- Indicators -->
	{#if showIndicators && totalSlides > 1}
		<div class="absolute bottom-4 left-1/2 -translate-x-1/2 flex gap-2">
			{#each slides as _, index}
				<button
					type="button"
					class="h-2 rounded-full transition-all duration-300 {index === currentIndex ? 'w-4 bg-white' : 'w-2 bg-white/50'}"
					onclick={() => goTo(index)}
					aria-label="Go to slide {index + 1}"
					aria-current={index === currentIndex ? 'true' : undefined}
				></button>
			{/each}
		</div>
	{/if}
</div>
