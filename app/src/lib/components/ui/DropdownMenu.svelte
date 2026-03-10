<!--
  DropdownMenu Component
  Dropdown menu with items, checkboxes, radio items, etc.
-->

<script lang="ts">
	import type { Snippet } from 'svelte';

	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	type IconComponent = any;

	interface MenuItem {
		label?: string;
		icon?: IconComponent;
		onClick?: () => void;
		onclick?: () => void; // alias for onClick
		separator?: boolean;
		divider?: boolean; // alias for separator
		disabled?: boolean;
		type?: 'item' | 'checkbox' | 'radio';
		variant?: 'default' | 'danger';
		checked?: boolean;
	}

	interface TriggerObject {
		icon?: IconComponent;
		class?: string;
	}

	interface Props {
		trigger?: Snippet | TriggerObject;
		items: MenuItem[];
		align?: 'start' | 'end';
		class?: string;
	}

	let {
		trigger,
		items = [],
		align = 'end',
		class: className = ''
	}: Props = $props();

	function handleItemClick(item: MenuItem, event: MouseEvent) {
		if (!item.disabled) {
			// Support both onClick and onclick
			(item.onClick || item.onclick)?.();
			if (item.type !== 'checkbox' && item.type !== 'radio') {
				// Close dropdown by removing focus
				const dropdown = (event.target as HTMLElement).closest('.dropdown');
				if (dropdown) {
					(dropdown.querySelector('[tabindex]') as HTMLElement)?.blur();
				}
			}
		}
	}

	// Check if trigger is a snippet (function) or an object
	function isSnippet(t: any): t is Snippet {
		return typeof t === 'function';
	}

	let dropdownClass = $derived(
		`dropdown dropdown-bottom ${align === 'end' ? 'dropdown-end' : ''} ${className}`.trim()
	);
</script>

<div class={dropdownClass}>
	{#if trigger}
		{#if isSnippet(trigger)}
			<div tabindex="0" role="button">
				{@render trigger()}
			</div>
		{:else}
			<!-- Object-based trigger with icon -->
			<button tabindex="0" type="button" class={trigger.class || 'btn btn-ghost btn-sm'}>
				{#if trigger.icon}
					<trigger.icon class="w-4 h-4" />
				{/if}
			</button>
		{/if}
	{/if}

	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<ul tabindex="0" class="dropdown-content menu bg-base-200 rounded-box z-50 min-w-48 p-2 shadow-lg">
		{#each items as item}
			{#if item.separator || item.divider}
				<li class="my-1"><hr class="border-base-300" /></li>
			{:else}
				<li class={item.disabled ? 'disabled' : ''}>
					<button
						type="button"
						onclick={(e) => handleItemClick(item, e)}
						disabled={item.disabled}
						class="flex items-center gap-2 {item.variant === 'danger' ? 'text-error hover:bg-error hover:text-error-content' : ''}"
						role="menuitem"
					>
						{#if item.type === 'checkbox' || item.type === 'radio'}
							<span class="w-4 h-4 flex items-center justify-center">
								{#if item.checked}
									{#if item.type === 'checkbox'}
										<svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
										</svg>
									{:else}
										<svg class="w-2 h-2 fill-current" viewBox="0 0 8 8">
											<circle cx="4" cy="4" r="4" />
										</svg>
									{/if}
								{/if}
							</span>
							<span>{item.label}</span>
						{:else}
							{#if item.icon}
								<span class="w-4 h-4">
									<item.icon class="w-4 h-4" />
								</span>
							{/if}
							<span>{item.label}</span>
						{/if}
					</button>
				</li>
			{/if}
		{/each}
	</ul>
</div>
