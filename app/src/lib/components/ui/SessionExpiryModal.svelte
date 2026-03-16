<!--
  Session Expiry Modal Component
  Matches NeboLoop.com modal style
-->

<script lang="ts">
	import { Clock, LogOut, X } from 'lucide-svelte';

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

{#if show}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="nebo-modal-backdrop"
		onkeydown={handleKeydown}
		role="dialog"
		aria-modal="true"
		aria-labelledby="session-expiry-title"
		tabindex="-1"
	>
		<div class="nebo-modal-overlay"></div>

		<div class="nebo-modal-card max-w-md">
			<!-- Header -->
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				<h3 id="session-expiry-title" class="font-display text-lg font-bold">Session Expiring Soon</h3>
				<button type="button" onclick={handleContinue} class="nebo-modal-close" aria-label="Close">
					<X class="w-5 h-5 text-base-content/80" />
				</button>
			</div>

			<!-- Body -->
			<div class="px-5 py-5 text-center">
				<div class="w-16 h-16 rounded-full bg-warning/20 flex items-center justify-center mx-auto mb-4">
					<Clock class="w-8 h-8 text-warning" />
				</div>
				<p class="text-base text-base-content/90 mb-4">
					Your session will expire in
				</p>
				<div class="text-4xl font-mono font-bold text-warning mb-4">
					{formattedTime()}
				</div>
				<p class="text-base text-base-content/90">
					Click "Continue Session" to stay logged in, or you'll be automatically logged out.
				</p>
			</div>

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
				<button
					type="button"
					class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors flex items-center gap-2"
					onclick={handleLogout}
				>
					<LogOut class="w-4 h-4" />
					Log Out Now
				</button>
				<button
					type="button"
					class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
					onclick={handleContinue}
				>
					Continue Session
				</button>
			</div>
		</div>
	</div>
{/if}
