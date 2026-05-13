<!--
  StatusBadge Component
  Context-aware status indicators with tooltips
-->

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Tooltip from './Tooltip.svelte';

	// Status config based on backend status definitions - maps to DaisyUI badge variants
	const statusConfig = $derived({
		// Collection statuses
		"ACTIVE": { variant: "badge-success", label: $t('statusBadge.active') },
		"active": { variant: "badge-success", label: $t('statusBadge.active') },
		"ERROR": { variant: "badge-error", label: $t('common.error') },
		"error": { variant: "badge-error", label: $t('common.error') },
		"NEEDS SOURCE": { variant: "badge-ghost", label: $t('statusBadge.needsSource') },
		"needs source": { variant: "badge-ghost", label: $t('statusBadge.needsSource') },
		// Source connection statuses
		"IN_PROGRESS": { variant: "badge-info", label: $t('statusBadge.syncing') },
		"in_progress": { variant: "badge-info", label: $t('statusBadge.inProgress') },
		"failing": { variant: "badge-error", label: $t('statusBadge.failing') },
		// Sync job statuses
		"pending": { variant: "badge-warning", label: $t('common.pending') },
		"completed": { variant: "badge-success", label: $t('common.completed') },
		"failed": { variant: "badge-error", label: $t('common.failed') },
		"cancelled": { variant: "badge-error", label: $t('common.cancelled') },
		// API Key statuses
		"EXPIRED": { variant: "badge-error", label: $t('statusBadge.expired') },
		"EXPIRING_SOON": { variant: "badge-warning", label: $t('statusBadge.expiringSoon') },
		"UNKNOWN": { variant: "badge-ghost", label: $t('common.unknown') },
		// Fallback for unknown statuses
		"default": { variant: "badge-ghost", label: $t('common.unknown') }
	});

	type TooltipContext = "collection" | "apiKey";

	interface Props {
		status: string;
		class?: string;
		showTooltip?: boolean;
		tooltipContext?: TooltipContext;
	}

	let {
		status,
		class: className = '',
		showTooltip = false,
		tooltipContext
	}: Props = $props();

	// Get status configuration or default
	function getStatusConfig(statusKey: string = "") {
		// Try exact match first
		if (statusKey in statusConfig) {
			return statusConfig[statusKey as keyof typeof statusConfig];
		}

		// Try case-insensitive match
		const lowerKey = statusKey.toLowerCase();
		for (const key in statusConfig) {
			if (key.toLowerCase() === lowerKey) {
				return statusConfig[key as keyof typeof statusConfig];
			}
		}

		// Return default if no match
		const formatted = statusKey ? statusKey.charAt(0).toUpperCase() + statusKey.slice(1).toLowerCase() : $t('common.unknown');
		return {
			...statusConfig["default"],
			label: formatted
		};
	}

	// Context-aware status descriptions for tooltips
	function getStatusDescription(statusKey: string, context?: TooltipContext): string | null {
		const normalizedKey = statusKey.toUpperCase();

		// Collection-specific descriptions
		const collectionDescriptions: Record<string, string> = {
			"ACTIVE": $t('statusBadge.collectionActive'),
			"ERROR": $t('statusBadge.collectionError'),
			"NEEDS SOURCE": $t('statusBadge.collectionNeedsSource')
		};

		// API key-specific descriptions
		const apiKeyDescriptions: Record<string, string> = {
			"ACTIVE": $t('statusBadge.apiKeyActive'),
			"EXPIRING_SOON": $t('statusBadge.apiKeyExpiring'),
			"EXPIRED": $t('statusBadge.apiKeyExpired'),
			"UNKNOWN": $t('statusBadge.apiKeyUnknown')
		};

		// Select description based on context
		if (context === "apiKey") {
			return apiKeyDescriptions[normalizedKey] || null;
		} else if (context === "collection") {
			return collectionDescriptions[normalizedKey] || null;
		}

		// No context specified - return null (no tooltip)
		return null;
	}

	const config = $derived(getStatusConfig(status));
	const description = $derived(getStatusDescription(status, tooltipContext));
</script>

{#if showTooltip && description}
	<Tooltip content={description}>
		<div class="badge badge-sm {config.variant} gap-1.5 {className}">
			<div class="h-2 w-2 rounded-full bg-current opacity-70"></div>
			<span>{config.label}</span>
		</div>
	</Tooltip>
{:else}
	<div class="badge badge-sm {config.variant} gap-1.5 {className}">
		<div class="h-2 w-2 rounded-full bg-current opacity-70"></div>
		<span>{config.label}</span>
	</div>
{/if}
