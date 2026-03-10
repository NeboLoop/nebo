<!--
  Skeleton Component
  Uses DaisyUI skeleton class
-->

<script lang="ts">
	interface Props {
		class?: string;
	}

	let {
		class: className = ''
	}: Props = $props();
</script>

<div class="skeleton {className}"></div>
