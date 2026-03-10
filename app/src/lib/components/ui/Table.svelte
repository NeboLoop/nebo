<!--
  Reusable Table Component
-->

<script lang="ts">
	interface Props {
		children: any;
		zebra?: boolean;
		pinRows?: boolean;
		pinCols?: boolean;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		class?: string;
	}

	let {
		children,
		zebra = false,
		pinRows = false,
		pinCols = false,
		size = 'md',
		class: className = ''
	}: Props = $props();

	const sizeClasses: Record<string, string> = {
		xs: 'table-xs',
		sm: 'table-sm',
		md: '',
		lg: 'table-lg'
	};

	let tableClass = $derived(
		`table ${sizeClasses[size]} ${zebra ? 'table-zebra' : ''} ${pinRows ? 'table-pin-rows' : ''} ${pinCols ? 'table-pin-cols' : ''} ${className}`.trim()
	);
</script>

<div class="overflow-x-auto rounded-lg bg-base-200">
	<table class={tableClass}>
		{@render children()}
	</table>
</div>
