<!--
  Breadcrumb Component
  Navigation breadcrumbs for hierarchy
-->

<script lang="ts">
	interface BreadcrumbItem {
		label: string;
		href?: string;
		icon?: string;
	}

	interface Props {
		items: BreadcrumbItem[];
		class?: string;
	}

	let {
		items,
		class: className = ''
	}: Props = $props();
</script>

<div class="breadcrumbs text-sm {className}">
	<ul>
		{#each items as item, index}
			<li>
				{#if item.href && index < items.length - 1}
					<a href={item.href} class="flex items-center gap-2">
						{#if item.icon}
							<i class="fas fa-{item.icon}"></i>
						{/if}
						{item.label}
					</a>
				{:else}
					<span class="flex items-center gap-2">
						{#if item.icon}
							<i class="fas fa-{item.icon}"></i>
						{/if}
						{item.label}
					</span>
				{/if}
			</li>
		{/each}
	</ul>
</div>
