<!--
  Badge Component
  For status indicators, counts, labels
-->

<script lang="ts">
	type BadgeVariant = 'default' | 'primary' | 'secondary' | 'accent' | 'success' | 'warning' | 'error' | 'info' | 'ghost' | 'neutral';

	interface Props {
		variant?: BadgeVariant;
		type?: BadgeVariant; // alias for variant
		size?: 'xs' | 'sm' | 'md' | 'lg';
		outline?: boolean;
		children: any;
		class?: string;
	}

	let {
		variant = 'default',
		type, // alias for variant
		size = 'md',
		outline = false,
		children,
		class: className = ''
	}: Props = $props();

	// Support both 'variant' and 'type' props
	const effectiveVariant = $derived(type || variant);

	const variantClasses: Record<string, string> = {
		default: 'badge-neutral',
		neutral: 'badge-neutral',
		primary: 'badge-primary',
		secondary: 'badge-secondary',
		accent: 'badge-accent',
		success: 'badge-success',
		warning: 'badge-warning',
		error: 'badge-error',
		info: 'badge-info',
		ghost: 'badge-ghost'
	};

	const sizeClasses: Record<string, string> = {
		xs: 'badge-xs',
		sm: 'badge-sm',
		md: 'badge-md',
		lg: 'badge-lg'
	};

	let badgeClass = $derived(
		`badge ${variantClasses[effectiveVariant]} ${sizeClasses[size]} ${outline ? 'badge-outline' : ''} ${className}`.trim()
	);
</script>

<span class={badgeClass}>
	{@render children()}
</span>
