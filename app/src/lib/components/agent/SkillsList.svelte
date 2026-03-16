<script lang="ts">
	let {
		skills,
		manageHref = '',
	}: {
		skills: string[];
		manageHref?: string;
	} = $props();
</script>

{#if skills.length > 0}
	<section class="pb-6 border-t border-base-content/10 pt-4">
		<div class="flex items-center justify-between mb-3">
			<h2 class="text-sm text-base-content/60 uppercase tracking-wider font-semibold">Skills</h2>
			{#if manageHref}
				<a href={manageHref} class="text-sm text-primary hover:underline">Manage</a>
			{/if}
		</div>
		<div class="flex flex-wrap gap-2">
			{#each skills as skill}
				<span class="text-sm px-3 py-1.5 rounded-lg bg-base-content/5 text-base-content/80">{skill}</span>
			{/each}
		</div>
	</section>
{/if}
