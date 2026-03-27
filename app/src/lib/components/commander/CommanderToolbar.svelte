<script lang="ts">
	import { LayoutGrid, ZoomIn, ZoomOut, Maximize2, Trash2 } from 'lucide-svelte';
	import { useSvelteFlow } from '@xyflow/svelte';
	import { t } from 'svelte-i18n';

	let {
		onAutoLayout = () => {},
		onDeleteSelected = () => {},
		hasSelection = false,
	} = $props<{
		onAutoLayout?: () => void;
		onDeleteSelected?: () => void;
		hasSelection?: boolean;
	}>();

	const { zoomIn, zoomOut, fitView } = useSvelteFlow();
</script>

<div class="commander-toolbar">
	<button type="button" class="btn btn-sm btn-ghost" onclick={onAutoLayout} title={$t('commander.autoLayout')}>
		<LayoutGrid size={16} />
	</button>
	<div class="w-px h-5 bg-base-content/10"></div>
	<button type="button" class="btn btn-sm btn-ghost" onclick={() => zoomIn()} title={$t('commander.zoomIn')}>
		<ZoomIn size={16} />
	</button>
	<button type="button" class="btn btn-sm btn-ghost" onclick={() => zoomOut()} title={$t('commander.zoomOut')}>
		<ZoomOut size={16} />
	</button>
	<button type="button" class="btn btn-sm btn-ghost" onclick={() => fitView()} title={$t('commander.fitView')}>
		<Maximize2 size={16} />
	</button>
	<div class="w-px h-5 bg-base-content/10"></div>
	<button
		type="button"
		class="btn btn-sm btn-ghost {hasSelection ? 'text-error' : 'text-base-content/20'}"
		onclick={onDeleteSelected}
		disabled={!hasSelection}
		title={hasSelection ? $t('commander.deleteConnection') : $t('commander.selectConnection')}
	>
		<Trash2 size={16} />
	</button>
</div>
