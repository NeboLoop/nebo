<!--
  Form Component
  Form wrapper with validation context (simplified for Svelte 5)

  Note: This is a simplified version. For complex forms, consider using
  a form library like svelte-forms-lib or felte
-->

<script lang="ts">
	interface Props {
		children: any;
		class?: string;
		onsubmit?: (e: SubmitEvent) => void;
	}

	let {
		children,
		class: className = '',
		onsubmit
	}: Props = $props();

	function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		onsubmit?.(e);
	}
</script>

<form onsubmit={handleSubmit} class={className}>
	{@render children()}
</form>
