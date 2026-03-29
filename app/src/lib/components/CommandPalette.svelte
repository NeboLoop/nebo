<!--
  Command Palette (Cmd+K)
  Global search overlay for quick navigation to pages, settings, skills, and actions.
-->

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { listExtensions } from '$lib/api/nebo';
	import type { ExtensionSkill } from '$lib/api/neboComponents';
	import { checkForUpdate } from '$lib/stores/update';
	import SearchInput from '$lib/components/ui/SearchInput.svelte';

	interface Props {
		open: boolean;
		onclose: () => void;
	}

	let { open = $bindable(), onclose }: Props = $props();

	let query = $state('');
	let selectedIndex = $state(0);
	let skills = $state<ExtensionSkill[]>([]);
	let skillsLoaded = $state(false);
	let skillsLoading = $state(false);
	let inputRef = $state<HTMLInputElement | null>(null);

	interface PaletteItem {
		category: string;
		label: string;
		description?: string;
		icon: string;
		action: () => void;
	}

	const navItems: PaletteItem[] = $derived([
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.dashboard'), icon: 'grid', action: () => goto('/') },
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.agents'), icon: 'cpu', action: () => goto('/agents') },
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.skills'), icon: 'zap', action: () => goto('/skills') },
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.integrations'), icon: 'plug', action: () => goto('/integrations') },
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.events'), icon: 'calendar', action: () => goto('/events') },
		{ category: $t('commandPalette.navigation'), label: $t('commandPalette.marketplace'), icon: 'store', action: () => goto('/marketplace') }
	]);

	const settingsItems: PaletteItem[] = $derived([
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.account'), icon: 'settings', action: () => goto('/settings/account') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.profile'), icon: 'settings', action: () => goto('/settings/profile') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.identity'), icon: 'settings', action: () => goto('/settings/identity') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.personality'), icon: 'settings', action: () => goto('/settings/soul') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.models'), icon: 'settings', action: () => goto('/settings/providers') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.permissions'), icon: 'settings', action: () => goto('/settings/permissions') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.heartbeat'), icon: 'settings', action: () => goto('/settings/heartbeat') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.sessions'), icon: 'settings', action: () => goto('/settings/sessions') },
		{ category: $t('commandPalette.settingsCategory'), label: $t('commandPalette.developer'), icon: 'settings', action: () => goto('/settings/developer') }
	]);

	const actionItems: PaletteItem[] = $derived([
		{ category: $t('commandPalette.actions'), label: $t('commandPalette.newChat'), icon: 'plus', action: () => goto('/agent/assistant/chat') },
		{ category: $t('commandPalette.actions'), label: $t('commandPalette.checkForUpdates'), icon: 'refresh', action: () => { checkForUpdate(); } }
	]);

	const skillItems = $derived<PaletteItem[]>(
		skills.map((s) => ({
			category: $t('commandPalette.skills'),
			label: s.name,
			description: s.description,
			icon: 'zap',
			action: () => goto('/skills')
		}))
	);

	const allItems = $derived([...navItems, ...settingsItems, ...skillItems, ...actionItems]);

	const filteredItems = $derived(() => {
		const q = query.toLowerCase().trim();
		if (!q) return allItems;
		return allItems.filter(
			(item) =>
				item.label.toLowerCase().includes(q) ||
				(item.description && item.description.toLowerCase().includes(q))
		);
	});

	const groupedResults = $derived(() => {
		const items = filteredItems().slice(0, 50);
		const groups: { category: string; items: PaletteItem[] }[] = [];
		for (const item of items) {
			const existing = groups.find((g) => g.category === item.category);
			if (existing) {
				existing.items.push(item);
			} else {
				groups.push({ category: item.category, items: [item] });
			}
		}
		return groups;
	});

	const flatFiltered = $derived(filteredItems().slice(0, 50));

	$effect(() => {
		if (open) {
			query = '';
			selectedIndex = 0;
			// Focus the input after it mounts
			requestAnimationFrame(() => {
				const el = document.querySelector<HTMLInputElement>('.command-palette-card input');
				if (el) el.focus();
			});
			// Lazy-load skills on first open
			if (!skillsLoaded && !skillsLoading) {
				skillsLoading = true;
				listExtensions()
					.then((res) => {
						skills = res.skills ?? [];
						skillsLoaded = true;
					})
					.catch(() => {
						skillsLoaded = true;
					})
					.finally(() => {
						skillsLoading = false;
					});
			}
		}
	});

	// Reset selection when query changes
	$effect(() => {
		query;
		selectedIndex = 0;
	});

	function handleKeydown(e: KeyboardEvent) {
		const items = flatFiltered;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			selectedIndex = items.length > 0 ? (selectedIndex + 1) % items.length : 0;
			scrollSelectedIntoView();
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			selectedIndex = items.length > 0 ? (selectedIndex - 1 + items.length) % items.length : 0;
			scrollSelectedIntoView();
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (items[selectedIndex]) {
				activate(items[selectedIndex]);
			}
		} else if (e.key === 'Escape') {
			e.preventDefault();
			close();
		}
	}

	function scrollSelectedIntoView() {
		requestAnimationFrame(() => {
			const el = document.querySelector('.command-palette-item[data-selected="true"]');
			if (el) el.scrollIntoView({ block: 'nearest' });
		});
	}

	function activate(item: PaletteItem) {
		close();
		item.action();
	}

	function close() {
		open = false;
		onclose();
	}

	function handleBackdropClick(e: MouseEvent) {
		if ((e.target as HTMLElement).classList.contains('command-palette-backdrop')) {
			close();
		}
	}

	const iconPaths: Record<string, string> = {
		grid: '<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/>',
		cpu: '<path d="M12 8V4H8"/><rect x="8" y="8" width="8" height="8" rx="1"/><path d="M12 16v4h4"/><path d="M8 12H4"/><path d="M20 12h-4"/>',
		user: '<circle cx="12" cy="8" r="5"/><path d="M20 21a8 8 0 0 0-16 0"/>',
		workflow: '<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/>',
		zap: '<path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/>',
		plug: '<path d="M12 2v4"/><path d="M12 18v4"/><path d="m4.93 4.93 2.83 2.83"/><path d="m16.24 16.24 2.83 2.83"/><path d="M2 12h4"/><path d="M18 12h4"/><path d="m4.93 19.07 2.83-2.83"/><path d="m16.24 7.76 2.83-2.83"/>',
		calendar: '<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/>',
		store: '<path d="M14.25 6.087c0-.355.186-.676.401-.959.221-.29.349-.634.349-1.003 0-1.036-1.007-1.875-2.25-1.875s-2.25.84-2.25 1.875c0 .369.128.713.349 1.003.215.283.401.604.401.959v0a.64.64 0 01-.657.643 48.39 48.39 0 01-4.163-.3c.186 1.613.293 3.25.315 4.907a.656.656 0 01-.658.663v0c-.355 0-.676-.186-.959-.401a1.647 1.647 0 00-1.003-.349c-1.036 0-1.875 1.007-1.875 2.25s.84 2.25 1.875 2.25c.369 0 .713-.128 1.003-.349.283-.215.604-.401.959-.401v0c.31 0 .555.26.532.57a48.039 48.039 0 01-.642 5.056c1.518.19 3.058.309 4.616.354a.64.64 0 00.657-.643v0c0-.355-.186-.676-.401-.959a1.647 1.647 0 01-.349-1.003c0-1.035 1.008-1.875 2.25-1.875 1.243 0 2.25.84 2.25 1.875 0 .369-.128.713-.349 1.003-.215.283-.4.604-.4.959v0c0 .333.277.599.61.58a48.1 48.1 0 005.427-.63 48.05 48.05 0 00.582-4.717.532.532 0 00-.533-.57v0c-.355 0-.676.186-.959.401-.29.221-.634.349-1.003.349-1.035 0-1.875-1.007-1.875-2.25s.84-2.25 1.875-2.25c.37 0 .713.128 1.003.349.283.215.604.401.96.401v0a.656.656 0 00.658-.663 48.422 48.422 0 00-.37-5.36c-1.886.342-3.81.574-5.766.689a.578.578 0 01-.61-.58v0z"/>',
		settings: '<path d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>',
		plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
		refresh: '<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/>'
	};
</script>

{#if open}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="command-palette-backdrop" onclick={handleBackdropClick} onkeydown={handleKeydown}>
		<div class="command-palette-card">
			<div class="px-4 pt-4 pb-3">
				<SearchInput
					bind:value={query}
					placeholder={$t('commandPalette.placeholder')}
					size="md"
					onkeydown={handleKeydown}
				/>
			</div>

			<hr class="border-base-content/10 m-0" />

			<div class="overflow-y-auto scrollbar-thin" style="max-height: 60vh;">
				{#if skillsLoading && !skillsLoaded}
					<div class="flex items-center justify-center py-6">
						<span class="loading loading-spinner loading-sm text-base-content/60"></span>
					</div>
				{/if}

				{#each groupedResults() as group}
					<div class="command-palette-category">{group.category}</div>
					{#each group.items as item, i}
						{@const globalIndex = flatFiltered.indexOf(item)}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							class="command-palette-item"
							data-selected={globalIndex === selectedIndex}
							onclick={() => activate(item)}
							onmouseenter={() => { selectedIndex = globalIndex; }}
						>
							<svg
								class="w-4 h-4 shrink-0 opacity-50"
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								{@html iconPaths[item.icon] ?? ''}
							</svg>
							<span class="truncate">{item.label}</span>
							{#if item.description}
								<span class="text-sm text-base-content/60 truncate ml-auto">{item.description}</span>
							{/if}
						</div>
					{/each}
				{/each}

				{#if groupedResults().length === 0 && !skillsLoading}
					<div class="px-4 py-8 text-center text-base text-base-content/60">
						{$t('common.noResultsFound')}
					</div>
				{/if}
			</div>

			<div class="command-palette-footer">
				<span><kbd class="kbd kbd-xs">&#8593;&#8595;</kbd> {$t('commandPalette.navigateHint')}</span>
				<span><kbd class="kbd kbd-xs">&#8629;</kbd> {$t('commandPalette.selectHint')}</span>
				<span><kbd class="kbd kbd-xs">esc</kbd> {$t('commandPalette.closeHint')}</span>
			</div>
		</div>
	</div>
{/if}
