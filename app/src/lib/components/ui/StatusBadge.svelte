<!--
  StatusBadge Component
  Context-aware status indicators with tooltips
-->

<script lang="ts">
	import Tooltip from './Tooltip.svelte';

	// Status config based on backend status definitions - maps to DaisyUI badge variants
	const statusConfig = {
		// Collection statuses
		"ACTIVE": { variant: "badge-success", label: "Active" },
		"active": { variant: "badge-success", label: "Active" },
		"ERROR": { variant: "badge-error", label: "Error" },
		"error": { variant: "badge-error", label: "Error" },
		"NEEDS SOURCE": { variant: "badge-ghost", label: "Needs Source" },
		"needs source": { variant: "badge-ghost", label: "Needs Source" },
		// Source connection statuses
		"IN_PROGRESS": { variant: "badge-info", label: "Syncing" },
		"in_progress": { variant: "badge-info", label: "In Progress" },
		"failing": { variant: "badge-error", label: "Failing" },
		// Sync job statuses
		"pending": { variant: "badge-warning", label: "Pending" },
		"completed": { variant: "badge-success", label: "Completed" },
		"failed": { variant: "badge-error", label: "Failed" },
		"cancelled": { variant: "badge-error", label: "Cancelled" },
		// API Key statuses
		"EXPIRED": { variant: "badge-error", label: "Expired" },
		"EXPIRING_SOON": { variant: "badge-warning", label: "Expiring Soon" },
		"UNKNOWN": { variant: "badge-ghost", label: "Unknown" },
		// Fallback for unknown statuses
		"default": { variant: "badge-ghost", label: "Unknown" }
	};

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
		const formatted = statusKey ? statusKey.charAt(0).toUpperCase() + statusKey.slice(1).toLowerCase() : "Unknown";
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
			"ACTIVE": "At least one source connection has completed a sync or is currently syncing. Your collection has data and is ready for queries.",
			"ERROR": "All source connections have failed their last sync. Check your connections and authentication to resolve sync issues.",
			"NEEDS SOURCE": "This collection has no authenticated connections, or connections exist but haven't successfully synced yet. Configure a source or wait for the initial sync to complete."
		};

		// API key-specific descriptions
		const apiKeyDescriptions: Record<string, string> = {
			"ACTIVE": "This API key is valid and can be used to authenticate requests. It will remain active until its expiration date.",
			"EXPIRING_SOON": "This API key will expire within 7 days. Create a new key before expiration to avoid service interruption.",
			"EXPIRED": "This API key has expired and can no longer be used. Delete this key and create a new one to continue using the API.",
			"UNKNOWN": "Unable to determine the status of this API key. Please verify the expiration date."
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
