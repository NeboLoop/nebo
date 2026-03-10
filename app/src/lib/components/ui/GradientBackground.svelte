<!--
  GradientBackground Component
  Consistent gradient background with optional noise texture
-->

<script lang="ts">
	interface Props {
		children: any;
		class?: string;
		withNoise?: boolean;
		noiseFrequency?: number;
		noiseOctaves?: number;
		noiseOpacity?: number;
	}

	let {
		children,
		class: className = '',
		withNoise = true,
		noiseFrequency = 1.2,
		noiseOctaves = 4,
		noiseOpacity
	}: Props = $props();

	let key = $state(Date.now());

	// Force re-render when noise parameters change
	$effect(() => {
		key = Date.now();
	});

	// Determine gradient classes based on theme
	const gradientClass = $derived(() => {
		if (typeof window !== 'undefined' && document.documentElement.classList.contains('dark')) {
			return "from-black via-black to-slate-900/10";
		} else {
			return "from-slate-100/40 via-slate-100/40 to-white";
		}
	});

	const defaultOpacity = $derived(() => {
		if (typeof window !== 'undefined' && document.documentElement.classList.contains('dark')) {
			return 0.05;
		} else {
			return 0.03;
		}
	});

	const finalOpacity = $derived(noiseOpacity !== undefined ? noiseOpacity : defaultOpacity());

	const noiseUrl = $derived(
		`data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noiseFilter'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='${noiseFrequency}' numOctaves='${noiseOctaves}' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noiseFilter)'/%3E%3C/svg%3E`
	);
</script>

<div class="bg-gradient-to-b {gradientClass()} {className} relative">
	{#if withNoise}
		{#key key}
			<div
				class="absolute inset-0 pointer-events-none z-0 noise-overlay"
				style:--noise-opacity={finalOpacity}
				style:--noise-url="url('{noiseUrl}')"
				aria-hidden="true"
			></div>
		{/key}
	{/if}
	<div class="relative z-10">
		{@render children()}
	</div>
</div>
