<!--
  Session Expiry Modal Component
  Shows when user session is about to expire, gives option to continue or logout
-->

<script lang="ts">
	import { Clock, LogOut } from 'lucide-svelte';
	import Button from './Button.svelte';

	interface Props {
		show?: boolean;
		secondsRemaining?: number;
		onContinue?: () => void;
		onLogout?: () => void;
	}

	let {
		show = $bindable(false),
		secondsRemaining = 60,
		onContinue,
		onLogout
	}: Props = $props();

	// Format seconds as MM:SS
	let formattedTime = $derived(() => {
		const mins = Math.floor(secondsRemaining / 60);
		const secs = secondsRemaining % 60;
		return `${mins}:${secs.toString().padStart(2, '0')}`;
	});

	function handleContinue() {
		show = false;
		onContinue?.();
	}

	function handleLogout() {
		show = false;
		onLogout?.();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			handleContinue();
		}
	}
</script>

<div
	class="modal"
	class:modal-open={show}
	onkeydown={handleKeydown}
	role="dialog"
	aria-modal="true"
	aria-labelledby="session-expiry-title"
	tabindex="-1"
>
	<div class="modal-box max-w-md">
		<!-- Icon and Title -->
		<div class="flex flex-col items-center text-center pb-4">
			<div class="w-16 h-16 rounded-full bg-warning/20 flex items-center justify-center mb-4">
				<Clock class="w-8 h-8 text-warning" />
			</div>
			<h3 id="session-expiry-title" class="text-xl font-bold">Session Expiring Soon</h3>
		</div>

		<!-- Body -->
		<div class="text-center py-4">
			<p class="text-base-content/70 mb-4">
				Your session will expire in
			</p>
			<div class="text-4xl font-mono font-bold text-warning mb-4">
				{formattedTime()}
			</div>
			<p class="text-sm text-base-content/60">
				Click "Continue Session" to stay logged in, or you'll be automatically logged out.
			</p>
		</div>

		<!-- Actions -->
		<div class="flex flex-col sm:flex-row gap-3 pt-4 border-t border-base-300">
			<Button
				type="ghost"
				onclick={handleLogout}
				class="flex-1 gap-2"
			>
				<LogOut class="w-4 h-4" />
				Log Out Now
			</Button>
			<Button
				type="primary"
				onclick={handleContinue}
				class="flex-1"
			>
				Continue Session
			</Button>
		</div>
	</div>

	<!-- Backdrop - don't allow closing by clicking outside -->
	<div class="modal-backdrop">
		<button class="cursor-default">close</button>
	</div>
</div>
