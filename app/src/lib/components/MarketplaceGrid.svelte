<script lang="ts">
	let { children } = $props<{ children: any }>();
</script>

<div class="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-px">
	{@render children()}
</div>
