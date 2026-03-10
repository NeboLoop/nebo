<!--
  Toast Notification Component
  Uses DaisyUI toast and alert classes
-->

<script lang="ts">
	import { Info, CheckCircle, AlertTriangle, XCircle, X } from 'lucide-svelte';

	interface Props {
		message: string;
		type?: 'success' | 'error' | 'warning' | 'info';
		duration?: number;
		show?: boolean;
		onclose?: () => void;
	}

	let {
		message,
		type = 'info',
		duration = 5000,
		show = $bindable(false),
		onclose
	}: Props = $props();

	let timeoutId: ReturnType<typeof setTimeout> | null = null;

	$effect(() => {
		if (show && duration > 0) {
			timeoutId = setTimeout(() => {
				handleClose();
			}, duration);
		}

		return () => {
			if (timeoutId) clearTimeout(timeoutId);
		};
	});

	function handleClose() {
		show = false;
		if (onclose) {
			onclose();
		}
	}

	const icons = {
		success: CheckCircle,
		error: XCircle,
		warning: AlertTriangle,
		info: Info
	};

	const alertClass = $derived(`alert alert-${type}`);
	const Icon = $derived(icons[type]);
</script>

{#if show}
	<div class="toast toast-end toast-bottom z-50">
		<div class={alertClass} role="alert">
			<Icon class="h-5 w-5 shrink-0" />
			<span>{message}</span>
			<button
				type="button"
				onclick={handleClose}
				class="btn btn-sm btn-ghost btn-circle"
				aria-label="Close notification"
			>
				<X class="h-4 w-4" />
			</button>
		</div>
	</div>
{/if}
