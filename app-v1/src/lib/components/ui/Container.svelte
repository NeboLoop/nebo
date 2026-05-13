<!--
  Container Component
  Wrapper for consistent max-width, padding, and centering
-->

<script lang="ts">
	interface Props {
		size?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
		padding?: 'none' | 'sm' | 'md' | 'lg';
		center?: boolean;
		as?: 'div' | 'section' | 'main' | 'article';
		class?: string;
		children: any;
	}

	let {
		size = 'lg',
		padding = 'md',
		center = true,
		as = 'div',
		class: extraClass = '',
		children
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
		sm: 'px-4 py-4',
		md: 'px-6 py-6',
		lg: 'px-8 py-8'
	};

	const className = $derived(
		[
			'w-full',
			sizeClasses[size],
			paddingClasses[padding],
			center ? 'mx-auto' : '',
			extraClass
		]
			.filter(Boolean)
			.join(' ')
	);
</script>

{#if as === 'section'}
	<section class={className}>
		{@render children()}
	</section>
{:else if as === 'main'}
	<main class={className}>
		{@render children()}
	</main>
{:else if as === 'article'}
	<article class={className}>
		{@render children()}
	</article>
{:else}
	<div class={className}>
		{@render children()}
	</div>
{/if}
