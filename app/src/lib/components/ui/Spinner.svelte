<!--
  Spinner Component
  Uses DaisyUI loading classes
-->

<script lang="ts">
	interface Props {
		class?: string;
		size?: number;
	}

	let {
		class: className = '',
		size = 16
	}: Props = $props();

	const sizeClass = $derived(
		size <= 12 ? 'loading-xs' :
		size <= 16 ? 'loading-sm' :
		size <= 24 ? 'loading-md' :
		'loading-lg'
	);
</script>

<span class="loading loading-spinner {sizeClass} {className}"></span>
