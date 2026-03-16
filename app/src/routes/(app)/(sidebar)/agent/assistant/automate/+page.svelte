<script lang="ts">
	import AutomationsSection from '$lib/components/agent/AutomationsSection.svelte';
</script>

<svelte:head>
	<title>Nebo - Assistant - Automate</title>
</svelte:head>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-3xl mx-auto px-6 py-6">
			<AutomationsSection
				entityType="main"
				entityId="main"
				readonly={true}
			/>

			<div class="mt-8 p-4 rounded-xl bg-base-content/5 text-center">
				<p class="text-sm text-base-content/60">
					Workflow automations are available on installed agents.
				</p>
				<a href="/marketplace?type=role" class="text-sm text-primary font-medium mt-1 block">
					Browse the marketplace
				</a>
			</div>
		</div>
	</div>
</div>
