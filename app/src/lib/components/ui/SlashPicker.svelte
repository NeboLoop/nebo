<script lang="ts">
	import { t } from 'svelte-i18n';
	import { tick } from 'svelte';
	import type { Resource } from '$lib/utils/resources';

	let {
		resources,
		query = '',
		selectedIdx = 0,
		onselect,
	}: {
		resources: Resource[];
		query?: string;
		selectedIdx?: number;
		onselect: (resource: Resource) => void;
	} = $props();

	const typeIcon: Record<string, string> = { mcp: '🔌', skill: '📄', agent: '🤖', cmd: '⚡' };
	const typeLabel: Record<string, string> = $derived({
		mcp: $t('slashPicker.mcp'),
		skill: $t('slashPicker.skill'),
		agent: $t('slashPicker.agent'),
		cmd: $t('slashPicker.command')
	});

	const filtered = $derived.by(() => {
		const q = query.toLowerCase();
		return resources.filter((r) => !q || r.name.toLowerCase().includes(q));
	});

	let scrollContainer: HTMLDivElement | undefined = $state();

	// Scroll selected item into view whenever selectedIdx changes
	$effect(() => {
		if (!scrollContainer) return;
		const idx = selectedIdx;
		tick().then(() => {
			const item = scrollContainer?.querySelector(`[data-idx="${idx}"]`);
			if (item) {
				item.scrollIntoView({ block: 'nearest' });
			}
		});
	});

	// Track whether list is scrollable and if scrolled to bottom
	let canScrollDown = $state(false);

	function checkScroll() {
		if (!scrollContainer) return;
		const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
		canScrollDown = scrollTop + clientHeight < scrollHeight - 4;
	}

	$effect(() => {
		if (!scrollContainer) return;
		// Check after render
		tick().then(checkScroll);
	});
</script>

{#if filtered.length > 0}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="w-64 rounded-xl bg-base-100 border border-base-content/10 shadow-xl overflow-hidden"
		onmousedown={(e) => e.preventDefault()}
	>
		<div class="px-3 py-2 border-b border-base-content/10 flex items-center gap-2">
			<span class="text-xs text-base-content/40 font-mono">/</span>
			<span class="text-xs text-base-content/40 flex-1 truncate">{query || $t('slashPicker.placeholder')}</span>
			<span class="text-xs text-base-content/25">{filtered.length}</span>
		</div>

		<div class="relative">
			<div
				bind:this={scrollContainer}
				class="slash-picker-list"
				onscroll={checkScroll}
			>
				{#each ['cmd', 'mcp', 'skill', 'agent'] as type}
					{@const group = filtered.filter((r) => r.type === type)}
					{#if group.length > 0}
						<div class="px-3 pt-2 pb-1">
							<p class="text-xs font-semibold text-base-content/40 uppercase tracking-wider mb-1">
								{typeLabel[type]}s
							</p>
							{#each group as resource}
								{@const idx = filtered.indexOf(resource)}
								<button
									type="button"
									data-idx={idx}
									class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-sm text-left
										{selectedIdx === idx ? 'bg-primary/10 text-primary' : ''}"
									onmousedown={() => onselect(resource)}
								>
									<span>{typeIcon[type]}</span>
									<span class="flex-1 truncate">{resource.name}</span>
									{#if resource.status === 'warn'}
										<span class="text-xs text-warning/70 shrink-0">{$t('slashPicker.notConnected')}</span>
									{/if}
								</button>
							{/each}
						</div>
					{/if}
				{/each}
			</div>

			<!-- Bottom fade gradient — hints more items below -->
			{#if canScrollDown}
				<div class="slash-picker-fade"></div>
			{/if}
		</div>

		<div class="px-3 py-1.5 border-t border-base-content/10">
			<p class="text-xs text-base-content/25">{$t('slashPicker.keyboardHint')}</p>
		</div>
	</div>
{/if}
