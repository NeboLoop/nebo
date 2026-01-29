<!--
  MarketingNav Component
  Navigation for the public marketing website (www)
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { Button } from '$lib/components/ui';

	interface NavItem {
		label: string;
		href: string;
	}

	let {
		items = [] as NavItem[]
	}: {
		items?: NavItem[];
	} = $props();

	const currentPath = $derived($page.url.pathname);

	let mobileMenuOpen = $state(false);

	function toggleMobileMenu() {
		mobileMenuOpen = !mobileMenuOpen;
	}

	function closeMobileMenu() {
		mobileMenuOpen = false;
	}
</script>

<header class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
	<nav class="flex items-center justify-between py-6" aria-label="Main navigation">
		<a
			href="/"
			class="font-display text-2xl font-black text-base-content tracking-tight hover:opacity-90 transition-opacity"
		>
			GoBot
		</a>

		<!-- Desktop Navigation -->
		<div class="hidden md:flex items-center gap-8">
			{#if items.length > 0}
				<ul class="flex items-center gap-6">
					{#each items as item}
						<li>
							<a
								href={item.href}
								class="text-[15px] font-medium transition-colors {currentPath === item.href
									? 'text-base-content'
									: 'text-base-content/70 hover:text-base-content'}"
							>
								{item.label}
							</a>
						</li>
					{/each}
				</ul>
			{/if}
			<div class="flex items-center gap-3">
				<Button type="ghost" size="sm" href="/auth/login">Login</Button>
				<Button type="primary" size="sm" href="/auth/register">Get Started</Button>
			</div>
		</div>

		<!-- Mobile Menu Button -->
		<button
			type="button"
			class="md:hidden flex items-center justify-center w-10 h-10 rounded-lg text-base-content/80 hover:text-base-content hover:bg-base-content/10 transition-colors"
			aria-label={mobileMenuOpen ? 'Close menu' : 'Open menu'}
			aria-expanded={mobileMenuOpen}
			onclick={toggleMobileMenu}
		>
			{#if mobileMenuOpen}
				<svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
				</svg>
			{:else}
				<svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path stroke-linecap="round" stroke-linejoin="round" d="M4 6h16M4 12h16M4 18h16" />
				</svg>
			{/if}
		</button>
	</nav>

	<!-- Mobile Menu -->
	{#if mobileMenuOpen}
		<div class="md:hidden pb-6 border-t border-base-content/10 pt-4 animate-fade-in">
			{#if items.length > 0}
				<ul class="space-y-2 mb-6">
					{#each items as item}
						<li>
							<a
								href={item.href}
								class="block py-2 text-[15px] font-medium transition-colors {currentPath ===
								item.href
									? 'text-base-content'
									: 'text-base-content/70 hover:text-base-content'}"
								onclick={closeMobileMenu}
							>
								{item.label}
							</a>
						</li>
					{/each}
				</ul>
			{/if}
			<div class="flex flex-col gap-3">
				<Button type="ghost" href="/auth/login" class="w-full">Login</Button>
				<Button type="primary" href="/auth/register" class="w-full">Get Started</Button>
			</div>
		</div>
	{/if}
</header>
