<!--
  Reusable Loading Spinner Component
  Uses DaisyUI loading classes
-->

<script lang="ts">
	let { size = 'normal' }: { size?: 'xs' | 'sm' | 'small' | 'normal' | 'md' | 'lg' | 'large' } = $props();

	const sizeClasses: Record<string, string> = {
		xs: 'loading-xs',
		sm: 'loading-sm',
		small: 'loading-sm',
		normal: 'loading-md',
		md: 'loading-md',
		lg: 'loading-lg',
		large: 'loading-lg'
	};

	const sizeClass = $derived(sizeClasses[size] || 'loading-md');
</script>

<div class="flex items-center justify-center p-4">
	<span class="loading loading-spinner {sizeClass}"></span>
</div>
