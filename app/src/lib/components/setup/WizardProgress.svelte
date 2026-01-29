<!--
  Wizard Progress Component
  Shows progress bar with step indicators for setup wizard
  Uses DaisyUI steps component
-->

<script lang="ts">
	import { Check } from 'lucide-svelte';

	let {
		currentStep,
		totalSteps,
		steps,
		class: extraClass = ''
	}: {
		currentStep: number;
		totalSteps: number;
		steps: string[];
		class?: string;
	} = $props();

	function getStepClass(index: number): string {
		if (index < currentStep) return 'step step-primary';
		if (index === currentStep) return 'step step-primary';
		return 'step';
	}

	function isCompleted(index: number): boolean {
		return index < currentStep;
	}

	function isCurrent(index: number): boolean {
		return index === currentStep;
	}
</script>

<div class="w-full {extraClass}">
	<ul class="steps steps-horizontal w-full">
		{#each steps as step, index}
			<li class={getStepClass(index)} data-content={isCompleted(index) ? '✓' : index + 1}>
				<span class="text-xs sm:text-sm" class:font-semibold={isCurrent(index)}>
					{step}
				</span>
			</li>
		{/each}
	</ul>
</div>
