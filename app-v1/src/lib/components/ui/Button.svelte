<!--
  Reusable Button Component using DaisyUI
-->

<script lang="ts">
	let {
		type = 'primary',
		variant, // alias for type
		size = 'md',
		disabled = false,
		active = false,
		outline = false,
		onclick,
		href,
		external = false,
		htmlType = 'button',
		title,
		children,
		class: extraClass = ''
	}: {
		type?: 'primary' | 'secondary' | 'link' | 'danger' | 'ghost';
		variant?: 'primary' | 'secondary' | 'link' | 'danger' | 'ghost';
		size?: 'xs' | 'sm' | 'md' | 'lg' | 'icon';
		disabled?: boolean;
		active?: boolean;
		outline?: boolean;
		onclick?: () => void;
		href?: string;
		external?: boolean;
		htmlType?: 'button' | 'submit' | 'reset';
		title?: string;
		children: any;
		class?: string;
	} = $props();

	// Support both 'type' and 'variant' props
	const btnType = $derived(variant || type);

	const typeClasses: Record<string, string> = {
		primary: 'btn-primary',
		secondary: 'btn-secondary',
		link: 'btn-link',
		danger: 'btn-error',
		ghost: 'btn-ghost'
	};

	const sizeClasses: Record<string, string> = {
		xs: 'btn-xs',
		sm: 'btn-sm',
		md: '',
		lg: 'btn-lg',
		icon: 'btn-square btn-sm'
	};

	const className = $derived(
		[
			'btn',
			typeClasses[btnType],
			sizeClasses[size],
			outline ? 'btn-outline' : '',
			active ? 'btn-active' : '',
			extraClass
		]
			.filter(Boolean)
			.join(' ')
	);
</script>

{#if href}
	<a
		{href}
		{title}
		class={className}
		class:btn-disabled={disabled}
		target={external ? '_blank' : undefined}
		rel={external ? 'noopener noreferrer' : undefined}
	>
		{@render children()}
	</a>
{:else}
	<button class={className} type={htmlType} {disabled} {onclick} {title}>
		{@render children()}
	</button>
{/if}
