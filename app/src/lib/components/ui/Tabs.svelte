<!--
  Tabs Component
  Tab navigation with modern styling using DaisyUI
-->

<script lang="ts">
	import type { ComponentType } from 'svelte';

	interface Tab {
		id: string;
		label: string;
		icon?: ComponentType;
		badge?: string | number;
	}

	interface Props {
		tabs: Tab[];
		activeTab?: string;
		onchange?: (tabId: string) => void;
		variant?: 'underline' | 'pills' | 'boxed';
	}

	let {
		tabs,
		activeTab = $bindable(''),
		onchange,
		variant = 'underline'
	}: Props = $props();

	function handleTabClick(tabId: string) {
		activeTab = tabId;
		if (onchange) {
			onchange(tabId);
		}
	}

	const variantClasses = {
		underline: 'tabs-bordered',
		pills: 'tabs-boxed bg-base-200 p-1 rounded-box',
		boxed: 'tabs-boxed bg-base-200 p-1 rounded-box'
	};
</script>

<div role="tablist" class="tabs {variantClasses[variant]}">
	{#each tabs as tab}
		<button
			type="button"
			role="tab"
			onclick={() => handleTabClick(tab.id)}
			class="tab gap-2 {activeTab === tab.id ? 'tab-active' : ''}"
			aria-selected={activeTab === tab.id}
		>
			{#if tab.icon}
				{@const IconComponent = tab.icon}
				<IconComponent class="w-4 h-4" />
			{/if}
			{tab.label}
			{#if tab.badge}
				<span class="badge badge-sm {activeTab === tab.id ? 'badge-primary' : 'badge-ghost'}">
					{tab.badge}
				</span>
			{/if}
		</button>
	{/each}
</div>
