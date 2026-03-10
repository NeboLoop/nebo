<script lang="ts" module>
	export type StepItem = {
		label: string;
		status: 'pending' | 'current' | 'complete';
	};
</script>

<script lang="ts">
	let {
		steps,
		vertical = false,
		class: className = ''
	}: {
		steps: StepItem[];
		vertical?: boolean;
		class?: string;
	} = $props();

	function getStepClass(status: StepItem['status']): string {
		if (status === 'complete') return 'step step-success';
		if (status === 'current') return 'step step-primary';
		return 'step';
	}
</script>

<ul class="steps {vertical ? 'steps-vertical' : 'steps-horizontal w-full'} {className}">
	{#each steps as step}
		<li class={getStepClass(step.status)}>{step.label}</li>
	{/each}
</ul>
