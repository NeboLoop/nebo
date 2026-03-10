<!--
  Error Boundary Component
  Catches errors in child components and displays a fallback UI
-->

<script lang="ts">
	import { AlertTriangle, RefreshCw } from 'lucide-svelte';
	import Button from './Button.svelte';

	interface Props {
		fallback?: any;
		onError?: (error: Error, errorInfo: string) => void;
		children: any;
	}

	let {
		fallback,
		onError,
		children
	}: Props = $props();

	let error = $state<Error | null>(null);
	let errorInfo = $state<string>('');

	// Reset error state
	function reset() {
		error = null;
		errorInfo = '';
	}

	// Handle errors from child components
	function handleError(e: ErrorEvent) {
		const err = e.error || new Error(e.message);
		error = err;
		errorInfo = e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : '';
		onError?.(err, errorInfo);
	}

	// Attach global error handler when mounted
	$effect(() => {
		if (typeof window !== 'undefined') {
			window.addEventListener('error', handleError);
			return () => window.removeEventListener('error', handleError);
		}
	});
</script>

{#if error}
	{#if fallback}
		{@render fallback()}
	{:else}
		<div class="flex flex-col items-center justify-center min-h-[200px] p-8 text-center">
			<div class="w-16 h-16 rounded-full bg-error/20 flex items-center justify-center mb-4">
				<AlertTriangle class="w-8 h-8 text-error" />
			</div>
			<h3 class="text-lg font-semibold mb-2">Something went wrong</h3>
			<p class="text-base-content/60 mb-4 max-w-md">
				An unexpected error occurred. Please try again or contact support if the problem persists.
			</p>
			{#if errorInfo}
				<p class="text-xs text-base-content/40 font-mono mb-4">
					{error.message}
				</p>
			{/if}
			<Button type="primary" onclick={reset} class="gap-2">
				<RefreshCw class="w-4 h-4" />
				Try Again
			</Button>
		</div>
	{/if}
{:else}
	{@render children()}
{/if}
