<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { auth, currentUser } from '$lib/stores';

	let isLoading = $state(true);
	let canRenderChildren = $state(false);

	const { children } = $props();

	onMount(() => {
		checkAuth();
	});

	async function checkAuth() {
		// Check localStorage directly for token
		const token = localStorage.getItem('nebo_token');

		if (!token) {
			// No token at all - redirect to login
			goto('/auth/login');
			return;
		}

		// Token exists - fetch user profile before showing content
		// This ensures AccountSettings and other components have user data
		await auth.fetchCurrentUser();

		isLoading = false;
		canRenderChildren = true;
	}
</script>

{#if isLoading}
	<div class="flex min-h-screen items-center justify-center bg-[var(--color-base-900)]">
		<div class="text-center">
			<div
				class="mx-auto h-12 w-12 animate-spin rounded-full border-b-2 border-[var(--color-accent-primary)]"
			></div>
			<p class="mt-4 text-sm text-[var(--color-base-400)]">Loading...</p>
		</div>
	</div>
{:else if canRenderChildren}
	{@render children()}
{/if}
