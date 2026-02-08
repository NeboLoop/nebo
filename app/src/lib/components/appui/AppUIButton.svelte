<!--
  AppUIButton - Small trigger button to open an app's UI panel.
  Displays an icon and the app name. Used in sidebars or headers.
-->

<script lang="ts">
	import { LayoutGrid } from 'lucide-svelte';
	import type { AppUIInfo } from '$lib/api/nebo';

	interface Props {
		app: AppUIInfo;
		onclick: () => void;
	}

	let { app, onclick }: Props = $props();
</script>

<button
	type="button"
	class="btn btn-ghost btn-sm gap-2 text-base-content/70 hover:text-base-content"
	{onclick}
	title="Open {app.name}"
>
	<LayoutGrid class="w-4 h-4" />
	<span class="truncate max-w-[120px]">{app.name}</span>
</button>
