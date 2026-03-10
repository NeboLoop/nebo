<!--
  Dropdown Menu Component
  Dropdown menu with items, dividers, and sections
-->

<script lang="ts">
	interface MenuItem {
		label: string;
		icon?: string;
		onclick?: () => void;
		href?: string;
		disabled?: boolean;
		danger?: boolean;
		divider?: boolean;
	}

	interface Props {
		items: MenuItem[];
		label?: string;
		icon?: string;
		align?: 'start' | 'end';
		position?: 'bottom' | 'top' | 'left' | 'right';
		children?: any;
		class?: string;
	}

	let {
		items,
		label = 'Options',
		icon = 'ellipsis-v',
		align = 'end',
		position = 'bottom',
		children,
		class: className = ''
	}: Props = $props();

	function handleItemClick(item: MenuItem, event: MouseEvent) {
		if (item.disabled) return;
		if (item.onclick) {
			item.onclick();
		}
		// Close dropdown by removing focus
		const dropdown = (event.target as HTMLElement).closest('.dropdown');
		if (dropdown) {
			(dropdown.querySelector('[tabindex]') as HTMLElement)?.blur();
		}
	}

	const positionClasses: Record<string, string> = {
		bottom: 'dropdown-bottom',
		top: 'dropdown-top',
		left: 'dropdown-left',
		right: 'dropdown-right'
	};

	let dropdownClass = $derived(
		`dropdown ${positionClasses[position]} ${align === 'end' ? 'dropdown-end' : ''} ${className}`.trim()
	);
</script>

<div class={dropdownClass}>
	<div tabindex="0" role="button" class="btn btn-ghost m-1">
		{#if children}
			{@render children()}
		{:else}
			<i class="fas fa-{icon}"></i>
			{label}
		{/if}
	</div>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<ul tabindex="0" class="dropdown-content menu bg-base-200 rounded-box z-50 w-56 p-2 shadow-lg">
		{#each items as item}
			{#if item.divider}
				<li class="divider my-1"></li>
			{:else if item.href}
				<li class={item.disabled ? 'disabled' : ''}>
					<a
						href={item.href}
						class={item.danger ? 'text-error hover:bg-error hover:text-error-content' : ''}
					>
						{#if item.icon}
							<i class="fas fa-{item.icon} w-5"></i>
						{/if}
						{item.label}
					</a>
				</li>
			{:else}
				<li class={item.disabled ? 'disabled' : ''}>
					<button
						type="button"
						onclick={(e) => handleItemClick(item, e)}
						disabled={item.disabled}
						class={item.danger ? 'text-error hover:bg-error hover:text-error-content' : ''}
					>
						{#if item.icon}
							<i class="fas fa-{item.icon} w-5"></i>
						{/if}
						{item.label}
					</button>
				</li>
			{/if}
		{/each}
	</ul>
</div>
