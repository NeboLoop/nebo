<!--
  Section Component
  Page section with optional title, subtitle, and consistent spacing
-->

<script lang="ts">
	interface Props {
		title?: string;
		subtitle?: string;
		size?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
		padding?: 'none' | 'sm' | 'md' | 'lg' | 'xl';
		background?: 'none' | 'base' | 'muted' | 'primary' | 'gradient';
		center?: boolean;
		class?: string;
		headerClass?: string;
		children: any;
		actions?: any;
	}

	let {
		title,
		subtitle,
		size = 'lg',
		padding = 'lg',
		background = 'none',
		center = false,
		class: extraClass = '',
		headerClass = '',
		children,
		actions
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		sm: 'max-w-2xl',
		md: 'max-w-4xl',
		lg: 'max-w-6xl',
		xl: 'max-w-7xl',
		full: 'max-w-full'
	};

	const paddingClasses: Record<string, string> = {
		none: '',
		sm: 'py-8',
		md: 'py-12',
		lg: 'py-16',
		xl: 'py-24'
	};

	const bgClasses: Record<string, string> = {
		none: '',
		base: 'bg-base-100',
		muted: 'bg-base-200',
		primary: 'bg-primary/5',
		gradient: 'bg-gradient-to-b from-base-200 to-base-100'
	};

	const sectionClass = $derived(
		[paddingClasses[padding], bgClasses[background], extraClass].filter(Boolean).join(' ')
	);

	const containerClass = $derived(
		['w-full mx-auto px-4 sm:px-6 lg:px-8', sizeClasses[size]].join(' ')
	);

	const headerAlignClass = $derived(center ? 'text-center' : '');
</script>

<section class={sectionClass}>
	<div class={containerClass}>
		{#if title || subtitle || actions}
			<div class="mb-8 {headerAlignClass} {headerClass}">
				<div class="flex items-start justify-between gap-4" class:flex-col={center} class:items-center={center}>
					<div>
						{#if title}
							<h2 class="text-2xl sm:text-3xl font-bold tracking-tight">{title}</h2>
						{/if}
						{#if subtitle}
							<p class="mt-2 text-base-content/60 max-w-2xl" class:mx-auto={center}>
								{subtitle}
							</p>
						{/if}
					</div>
					{#if actions}
						<div class="flex-shrink-0" class:mt-4={center}>
							{@render actions()}
						</div>
					{/if}
				</div>
			</div>
		{/if}

		{@render children()}
	</div>
</section>
