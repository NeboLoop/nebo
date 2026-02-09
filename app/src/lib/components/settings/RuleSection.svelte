<script lang="ts">
	import { ChevronDown, ChevronRight, X, Plus, Lock } from 'lucide-svelte';
	import { generateUUID } from '$lib/utils';

	interface RuleItem {
		id: string;
		text: string;
		enabled: boolean;
	}

	interface Section {
		id: string;
		name: string;
		items: RuleItem[];
	}

	let {
		section,
		onUpdate,
		onDelete,
		readonly = false,
		itemLabel = 'rule'
	}: {
		section: Section;
		onUpdate: (section: Section) => void;
		onDelete: (sectionId: string) => void;
		readonly?: boolean;
		itemLabel?: string;
	} = $props();

	let collapsed = $state(false);
	let editingName = $state(false);
	let editingItemId = $state<string | null>(null);
	let nameInput = $state('');
	let itemInput = $state('');
	let addingItem = $state(false);
	let newItemText = $state('');

	function startEditName() {
		if (readonly) return;
		nameInput = section.name;
		editingName = true;
	}

	function saveName() {
		if (nameInput.trim()) {
			onUpdate({ ...section, name: nameInput.trim() });
		}
		editingName = false;
	}

	function cancelEditName() {
		editingName = false;
	}

	function toggleItem(itemId: string) {
		if (readonly) return;
		const items = section.items.map((item) =>
			item.id === itemId ? { ...item, enabled: !item.enabled } : item
		);
		onUpdate({ ...section, items });
	}

	function startEditItem(item: RuleItem) {
		if (readonly) return;
		editingItemId = item.id;
		itemInput = item.text;
	}

	function saveItem(itemId: string) {
		if (itemInput.trim()) {
			const items = section.items.map((item) =>
				item.id === itemId ? { ...item, text: itemInput.trim() } : item
			);
			onUpdate({ ...section, items });
		}
		editingItemId = null;
	}

	function cancelEditItem() {
		editingItemId = null;
	}

	function deleteItem(itemId: string) {
		const items = section.items.filter((item) => item.id !== itemId);
		onUpdate({ ...section, items });
	}

	function startAddItem() {
		newItemText = '';
		addingItem = true;
	}

	function confirmAddItem() {
		if (newItemText.trim()) {
			const newItem: RuleItem = {
				id: generateUUID(),
				text: newItemText.trim(),
				enabled: true
			};
			onUpdate({ ...section, items: [...section.items, newItem] });
		}
		addingItem = false;
		newItemText = '';
	}

	function cancelAddItem() {
		addingItem = false;
		newItemText = '';
	}

	function handleNameKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') saveName();
		if (e.key === 'Escape') cancelEditName();
	}

	function handleItemKeydown(e: KeyboardEvent, itemId: string) {
		if (e.key === 'Enter') saveItem(itemId);
		if (e.key === 'Escape') cancelEditItem();
	}

	function handleNewItemKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') confirmAddItem();
		if (e.key === 'Escape') cancelAddItem();
	}
</script>

<div class="rule-section rounded-xl border border-base-300 bg-base-100 overflow-hidden">
	<!-- Section Header -->
	<button
		type="button"
		class="rule-section-header w-full flex items-center gap-2 px-4 py-3 hover:bg-base-200/50 transition-colors"
		onclick={() => (collapsed = !collapsed)}
	>
		<span class="text-base-content/40">
			{#if collapsed}
				<ChevronRight class="w-4 h-4" />
			{:else}
				<ChevronDown class="w-4 h-4" />
			{/if}
		</span>

		{#if editingName}
			<!-- svelte-ignore a11y_autofocus -->
			<input
				class="input input-bordered input-xs flex-1 text-sm font-semibold"
				bind:value={nameInput}
				onkeydown={handleNameKeydown}
				onblur={saveName}
				onclick={(e) => e.stopPropagation()}
				autofocus
			/>
		{:else}
			<span
				class="flex-1 text-left text-sm font-semibold text-base-content"
				ondblclick={(e) => {
					e.stopPropagation();
					startEditName();
				}}
			>
				{section.name}
			</span>
		{/if}

		{#if readonly}
			<Lock class="w-3.5 h-3.5 text-base-content/30" />
		{:else}
			<button
				type="button"
				class="rule-section-delete text-base-content/15 hover:text-error transition-colors"
				onclick={(e) => {
					e.stopPropagation();
					onDelete(section.id);
				}}
			>
				<X class="w-4 h-4" />
			</button>
		{/if}
	</button>

	<!-- Items -->
	{#if !collapsed}
		<div class="rule-section-items px-4 pb-3 space-y-1">
			{#each section.items as item (item.id)}
				<div class="rule-item flex items-start gap-2 group/item py-1">
					{#if !readonly}
						<label class="rule-item-toggle swap mt-0.5 cursor-pointer">
							<input
								type="checkbox"
								checked={item.enabled}
								onchange={() => toggleItem(item.id)}
							/>
							<div
								class="w-4 h-4 rounded border-2 flex items-center justify-center transition-colors
								{item.enabled
									? 'bg-primary border-primary'
									: 'bg-base-200 border-base-300'}"
							>
								{#if item.enabled}
									<svg class="w-2.5 h-2.5 text-primary-content" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="3">
										<path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
									</svg>
								{/if}
							</div>
						</label>
					{/if}

					{#if editingItemId === item.id}
						<!-- svelte-ignore a11y_autofocus -->
						<input
							class="input input-bordered input-xs flex-1 text-sm"
							bind:value={itemInput}
							onkeydown={(e) => handleItemKeydown(e, item.id)}
							onblur={() => saveItem(item.id)}
							autofocus
						/>
					{:else}
						<span
							class="flex-1 text-sm leading-relaxed transition-colors
							{item.enabled ? 'text-base-content' : 'text-base-content/30 line-through'}
							{readonly ? '' : 'cursor-pointer hover:text-primary'}"
							ondblclick={() => startEditItem(item)}
						>
							{item.text}
						</span>
					{/if}

					{#if !readonly}
						<button
							type="button"
							class="rule-item-delete text-base-content/15 hover:text-error transition-colors mt-0.5 shrink-0"
							onclick={() => deleteItem(item.id)}
						>
							<X class="w-3.5 h-3.5" />
						</button>
					{/if}
				</div>
			{/each}

			{#if !readonly}
				{#if addingItem}
					<div class="flex items-center gap-2 py-1">
						<!-- svelte-ignore a11y_autofocus -->
						<input
							class="input input-bordered input-xs flex-1 text-sm"
							placeholder="Type a {itemLabel}..."
							bind:value={newItemText}
							onkeydown={handleNewItemKeydown}
							onblur={confirmAddItem}
							autofocus
						/>
					</div>
				{:else}
					<button
						type="button"
						class="flex items-center gap-1.5 text-xs text-base-content/40 hover:text-primary transition-colors py-1"
						onclick={startAddItem}
					>
						<Plus class="w-3.5 h-3.5" />
						Add {itemLabel}
					</button>
				{/if}
			{/if}
		</div>
	{/if}
</div>
