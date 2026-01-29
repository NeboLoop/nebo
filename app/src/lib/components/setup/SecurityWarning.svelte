<!--
  Security Warning Component
  Prominent warning box about GoBot's capabilities
  Uses DaisyUI alert alert-warning
-->

<script lang="ts">
	import { ShieldAlert } from 'lucide-svelte';

	let {
		class: extraClass = ''
	}: {
		class?: string;
	} = $props();
</script>

<div class="alert alert-warning {extraClass}">
	<ShieldAlert class="h-6 w-6 shrink-0" />
	<div class="flex flex-col gap-2">
		<h3 class="font-bold">Important: GoBot has powerful capabilities</h3>
		<div class="text-sm">
			<p class="mb-2">When enabled, GoBot can:</p>
			<ul class="list-disc list-inside space-y-1 ml-2">
				<li><strong>File Access</strong> - Read, write, and modify files on your system</li>
				<li><strong>Shell Commands</strong> - Execute terminal commands and scripts</li>
				<li><strong>Network Access</strong> - Make HTTP requests and browse the web</li>
			</ul>
			<p class="mt-3">
				You can configure approval requirements and permissions in the next steps to control what actions require your confirmation.
			</p>
		</div>
	</div>
</div>
