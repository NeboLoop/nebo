<script lang="ts">
	let { data } = $props<{ data: { name: string; color?: string } }>();
</script>

<div class="commander-team-group" style:border-color={data.color ?? '#6366f1'}>
	<div class="commander-team-label" style:color={data.color ?? '#6366f1'}>
		{data.name}
	</div>
</div>
