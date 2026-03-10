<!--
  Reusable Alert Component
  Uses DaisyUI alert classes
-->

<script lang="ts">
	import {
		Info,
		CheckCircle,
		AlertTriangle,
		XCircle,
		ShieldAlert,
		Lightbulb,
		TrendingUp,
		Scale
	} from 'lucide-svelte';
	import type { ComponentType } from 'svelte';

	let {
		type = 'info',
		variant, // alias for type
		icon,
		title,
		dismissible = false,
		dismissable, // alias for dismissible
		onclose,
		children,
		class: extraClass = ''
	}: {
		type?: 'info' | 'success' | 'warning' | 'error' | 'security';
		variant?: 'info' | 'success' | 'warning' | 'error' | 'security';
		icon?: ComponentType | string;
		title?: string;
		dismissible?: boolean;
		dismissable?: boolean;
		onclose?: () => void;
		children: any;
		class?: string;
	} = $props();

	// Support both 'type' and 'variant' props
	const alertType = $derived(variant || type);
	// Support both 'dismissible' and 'dismissable' props, or infer from onclose
	const canDismiss = $derived(dismissible || dismissable || !!onclose);
	let visible = $state(true);

	function handleDismiss() {
		visible = false;
		onclose?.();
	}

	const defaultIcons = {
		info: Info,
		success: CheckCircle,
		warning: AlertTriangle,
		error: XCircle,
		security: ShieldAlert
	};

	const iconNameMap: Record<string, ComponentType> = {
		'exclamation-triangle': AlertTriangle,
		lightbulb: Lightbulb,
		'chart-line': TrendingUp,
		'info-circle': Info,
		'balance-scale': Scale
	};

	const Icon = $derived(
		!icon
			? defaultIcons[alertType]
			: typeof icon === 'string'
				? iconNameMap[icon] || defaultIcons[alertType]
				: icon
	);

	const alertClass = $derived(
		alertType === 'security' ? 'alert alert-warning' : `alert alert-${alertType}`
	);

	const iconColorClass = $derived({
		info: 'text-info',
		success: 'text-success',
		warning: 'text-warning',
		error: 'text-error',
		security: 'text-warning'
	}[alertType]);
</script>

{#if visible}
	<div class="{alertClass} {canDismiss ? 'pr-12' : ''} {extraClass}">
		<Icon class="h-6 w-6 shrink-0 {iconColorClass}" />
		<div class="flex flex-col gap-1">
			{#if title}
				<h3 class="font-semibold">{title}</h3>
			{/if}
			<div class="text-sm">
				{@render children()}
			</div>
		</div>
		{#if canDismiss}
			<button
				type="button"
				onclick={handleDismiss}
				class="btn btn-ghost btn-sm btn-circle absolute right-2 top-2"
			>
				<XCircle class="h-4 w-4" />
			</button>
		{/if}
	</div>
{/if}
