<!--
  CollapsibleCard Component
  Card with collapsible content using DaisyUI collapse
-->

<script lang="ts">
	interface Props {
		children: any;
		header: any;
		isExpanded?: boolean;
		onToggle?: (expanded: boolean) => void;
		class?: string;
		onCopy?: () => Promise<void>;
		copyTooltip?: string;
		autoExpandOnSearch?: boolean;
		statusRibbon?: any;
	}

	let {
		children,
		header,
		isExpanded = $bindable(true),
		onToggle,
		class: className = '',
		onCopy,
		copyTooltip = "Copy",
		autoExpandOnSearch = false,
		statusRibbon
	}: Props = $props();

	let copied = $state(false);

	function handleToggle() {
		const newExpanded = !isExpanded;
		isExpanded = newExpanded;
		onToggle?.(newExpanded);
	}

	async function handleCopy() {
		if (!onCopy) return;

		try {
			await onCopy();
			copied = true;
			setTimeout(() => copied = false, 2000);
		} catch (error) {
			console.error('Copy failed:', error);
		}
	}
</script>

<div class="card bg-base-100 shadow-xl {className}">
	<!-- Status Ribbon -->
	{#if statusRibbon}
		{@render statusRibbon()}
	{/if}

	<div class="collapse {isExpanded ? 'collapse-open' : 'collapse-close'}">
		<!-- Header - Always Visible -->
		<div class="collapse-title p-0 min-h-0">
			<div class="flex items-center justify-between px-4 py-2 bg-base-200 border-b border-base-300">
				<!-- Header Content -->
				<div class="flex items-center gap-2 flex-1">
					{@render header()}
				</div>

				<!-- Controls -->
				<div class="flex items-center gap-2">
					<!-- Copy Button -->
					{#if onCopy}
						<button
							type="button"
							onclick={handleCopy}
							class="btn btn-ghost btn-sm gap-1"
							title={copyTooltip}
						>
							{#if copied}
								<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
									<path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
								</svg>
							{:else}
								<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
									<path stroke-linecap="round" stroke-linejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
								</svg>
							{/if}
							Copy
						</button>
					{/if}

					<!-- Collapse/Expand Button -->
					<button
						type="button"
						onclick={handleToggle}
						title={isExpanded ? "Collapse" : "Expand"}
						aria-label={isExpanded ? "Collapse" : "Expand"}
						class="btn btn-ghost btn-sm btn-square"
					>
						<svg
							class="h-4 w-4 transition-transform duration-200 {isExpanded ? 'rotate-180' : 'rotate-0'}"
							fill="none"
							viewBox="0 0 24 24"
							stroke="currentColor"
						>
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
						</svg>
					</button>
				</div>
			</div>
		</div>

		<!-- Collapsible Content -->
		<div class="collapse-content p-0">
			<div class="p-4">
				{@render children()}
			</div>
		</div>
	</div>
</div>
