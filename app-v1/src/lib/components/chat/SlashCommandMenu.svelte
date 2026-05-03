<!--
  Slash Command Menu
  Floating dropdown that appears above the textarea when user types "/".
-->

<script lang="ts">
	import { getSlashCommandCompletions, type SlashCommand } from './slash-commands';

	interface Props {
		query: string;
		visible: boolean;
		onselect: (command: SlashCommand) => void;
		onclose: () => void;
	}

	let { query, visible, onselect, onclose }: Props = $props();

	let selectedIndex = $state(0);

	const completions = $derived(getSlashCommandCompletions(query));

	// Group by category
	const grouped = $derived.by(() => {
		const groups: { category: string; items: SlashCommand[] }[] = [];
		for (const cmd of completions) {
			const existing = groups.find((g) => g.category === cmd.category);
			if (existing) {
				existing.items.push(cmd);
			} else {
				groups.push({ category: cmd.category, items: [cmd] });
			}
		}
		return groups;
	});

	// Reset selection when query changes
	$effect(() => {
		query;
		selectedIndex = 0;
	});

	// Close if no completions
	$effect(() => {
		if (visible && completions.length === 0) {
			onclose();
		}
	});

	export function navigate(direction: 'up' | 'down') {
		if (completions.length === 0) return;
		if (direction === 'down') {
			selectedIndex = (selectedIndex + 1) % completions.length;
		} else {
			selectedIndex = (selectedIndex - 1 + completions.length) % completions.length;
		}
		scrollSelectedIntoView();
	}

	export function selectCurrent(): SlashCommand | null {
		if (completions[selectedIndex]) {
			return completions[selectedIndex];
		}
		return null;
	}

	function scrollSelectedIntoView() {
		requestAnimationFrame(() => {
			const el = document.querySelector('.slash-command-item.selected');
			if (el) el.scrollIntoView({ block: 'nearest' });
		});
	}
</script>

{#if visible && completions.length > 0}
	<div class="slash-command-menu scrollbar-thin">
		{#each grouped as group}
			<div class="slash-command-category">{group.category}</div>
			{#each group.items as cmd}
				{@const globalIndex = completions.indexOf(cmd)}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<div
					class="slash-command-item {globalIndex === selectedIndex ? 'selected' : ''}"
					onmouseenter={() => { selectedIndex = globalIndex; }}
					onclick={() => onselect(cmd)}
				>
					<div class="flex flex-col min-w-0">
						<div class="flex items-baseline gap-1">
							<span class="slash-command-name">/{cmd.name}</span>
							{#if cmd.args}
								<span class="slash-command-args">{cmd.args}</span>
							{/if}
						</div>
						<span class="slash-command-desc">{cmd.description}</span>
					</div>
				</div>
			{/each}
		{/each}
	</div>
{/if}
