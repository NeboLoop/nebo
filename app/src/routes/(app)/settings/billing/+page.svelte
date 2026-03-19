<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { ExternalLink, Check, CreditCard, Receipt, AlertTriangle, Gift, Copy, Info } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type {
		NeboLoopAccountStatusResponse,
		BillingPriceInfo,
		PaymentMethodInfo,
		InvoiceInfo,
		NeboLoopJanusUsageResponse
	} from '$lib/api/neboComponents';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';

	let isLoading = $state(true);
	let status = $state<NeboLoopAccountStatusResponse | null>(null);
	let subscription = $state<{ plan: string; subscriptions: any[] } | null>(null);
	let paymentMethods = $state<PaymentMethodInfo[]>([]);
	let invoices = $state<InvoiceInfo[]>([]);
	let usage = $state<NeboLoopJanusUsageResponse | null>(null);
	let referralCode = $state('');
	let referralLink = $state('');
	let referralCopied = $state(false);
	let referralLinkCopied = $state(false);
	let showGiftInfo = $state(false);
	let actionLoading = $state('');
	let actionError = $state('');

	// Delete account modal
	let showDeleteModal = $state(false);
	let deleteConfirmText = $state('');
	let deleteLoading = $state(false);
	const canDelete = $derived(deleteConfirmText === 'DELETE');

	onMount(async () => {
		try {
			status = await api.neboLoopAccountStatus();
			if (status?.connected) {
				const [subResp, pmResp, invResp, usageResp, referralResp] = await Promise.allSettled([
					api.neboLoopBillingSubscription(),
					api.neboLoopBillingPaymentMethods(),
					api.neboLoopBillingInvoices(),
					api.neboLoopJanusUsage(),
					api.neboLoopReferralCode()
				]);
				if (subResp.status === 'fulfilled') subscription = subResp.value;
				if (pmResp.status === 'fulfilled') paymentMethods = pmResp.value?.methods || [];
				if (invResp.status === 'fulfilled') invoices = invResp.value?.invoices || [];
				if (usageResp.status === 'fulfilled') usage = usageResp.value;
				if (referralResp.status === 'fulfilled') {
					referralCode = referralResp.value.referral_code;
					referralLink = referralResp.value.referral_link;
				}
			}
		} catch {
			status = null;
		} finally {
			isLoading = false;
		}

		const handler = (e: Event) => {
			const detail = (e as CustomEvent).detail;
			if (detail?.plan && status) {
				status = { ...status, plan: detail.plan };
			}
		};
		window.addEventListener('nebo:plan_changed', handler);
		return () => window.removeEventListener('nebo:plan_changed', handler);
	});

	const currentPlan = $derived((subscription?.plan || status?.plan || 'free').toLowerCase());
	const planName = $derived(currentPlan.charAt(0).toUpperCase() + currentPlan.slice(1));

	function formatTokens(n: number): string {
		if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
		if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
		return n.toLocaleString();
	}

	function timeUntilReset(resetAt?: string): string {
		if (!resetAt) return '';
		const diff = new Date(resetAt).getTime() - Date.now();
		if (diff <= 0) return 'resetting...';
		const h = Math.floor(diff / 3600000);
		const m = Math.floor((diff % 3600000) / 60000);
		if (h > 24) {
			const d = Math.floor(h / 24);
			return `resets in ${d}d`;
		}
		return `resets in ${h}h ${m}m`;
	}

	function formatPrice(amountCents: number, currency: string): string {
		return new Intl.NumberFormat('en-US', {
			style: 'currency',
			currency: currency || 'usd',
			minimumFractionDigits: 0
		}).format(amountCents / 100);
	}

	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric',
			year: 'numeric'
		});
	}

	async function handlePortal() {
		actionLoading = 'portal';
		actionError = '';
		try {
			await api.neboLoopBillingPortal();
		} catch (e: any) {
			actionError = e?.message || 'Failed to open payment portal. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	async function handleCancel(subscriptionId: string) {
		actionLoading = 'cancel';
		actionError = '';
		try {
			await api.neboLoopBillingCancel(subscriptionId);
			try {
				subscription = await api.neboLoopBillingSubscription();
			} catch { /* ignore */ }
		} catch (e: any) {
			actionError = e?.message || 'Failed to cancel subscription. Please try again.';
		} finally {
			actionLoading = '';
		}
	}

	async function handleDeleteAccount() {
		if (!canDelete) return;
		deleteLoading = true;
		try {
			await api.deleteAccount({ password: '' });
			await api.neboLoopDisconnect();
			showDeleteModal = false;
			window.location.href = '/';
		} catch (e: any) {
			actionError = e?.message || 'Failed to delete account. Please try again.';
		} finally {
			deleteLoading = false;
		}
	}

	function openDeleteModal() {
		deleteConfirmText = '';
		actionError = '';
		showDeleteModal = true;
	}

	function copyReferralCode() {
		navigator.clipboard.writeText(referralCode);
		referralCopied = true;
		setTimeout(() => referralCopied = false, 2000);
	}

	function copyReferralLink() {
		navigator.clipboard.writeText(referralLink);
		referralLinkCopied = true;
		setTimeout(() => referralLinkCopied = false, 2000);
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Billing</h2>
	<p class="text-base text-base-content/80">Manage your subscription, usage, and payment method</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading billing...</span>
	</div>
{:else if !status?.connected}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
		<p class="text-base text-base-content/80">Connect your NeboLoop account to manage billing.</p>
		<a href="/settings/account" class="inline-block mt-3 text-base font-medium text-primary hover:brightness-110 transition-all">
			Go to Account
		</a>
	</div>
{:else}
	<div class="space-y-8">
		<!-- Error Banner -->
		{#if actionError}
			<div class="rounded-2xl bg-error/10 border border-error/20 p-4 flex items-start gap-3">
				<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
				<div class="flex-1">
					<p class="text-base text-error">{actionError}</p>
				</div>
				<button onclick={() => (actionError = '')} class="text-base text-error/70 hover:text-error">Dismiss</button>
			</div>
		{/if}

		<!-- Current Plan -->
		<section>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-lg font-bold text-base-content">{planName} plan</p>
						<p class="text-base text-base-content/80 mt-0.5">
							{#if currentPlan === 'free'}
								Get started with Nebo
							{:else if currentPlan === 'pro'}
								For individuals who use Nebo daily
							{:else if currentPlan === 'max'}
								Maximum power for professionals
							{:else if currentPlan === 'team'}
								Collaboration for teams
							{:else}
								Your current plan
							{/if}
						</p>
					</div>
					<button
						onclick={() => goto('/upgrade')}
						class="h-9 px-4 rounded-xl text-base font-bold transition-all
							{currentPlan === 'free'
								? 'bg-primary text-primary-content hover:brightness-110'
								: 'border border-base-content/10 text-base-content hover:bg-base-content/5'}"
					>
						{currentPlan === 'free' ? 'Upgrade' : 'Change plan'}
					</button>
				</div>
			</div>
		</section>

		<!-- Usage -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Usage</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4 space-y-3">
				{#if usage?.session}
					<div>
						<div class="flex items-center justify-between mb-1.5">
							<span class="text-sm font-medium text-base-content/80">Session</span>
							<span class="text-sm text-base-content/60 tabular-nums">
								{formatTokens(usage.session.usedTokens)} / {formatTokens(usage.session.limitTokens)}
								{#if usage.session.resetAt}
									<span class="ml-1">&middot; {timeUntilReset(usage.session.resetAt)}</span>
								{/if}
							</span>
						</div>
						<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
							<div
								class="h-full rounded-full transition-all {usage.session.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {usage.session.percentUsed}%"
							></div>
						</div>
					</div>
				{/if}
				{#if usage?.weekly}
					<div>
						<div class="flex items-center justify-between mb-1.5">
							<span class="text-sm font-medium text-base-content/80">Weekly</span>
							<span class="text-sm text-base-content/60 tabular-nums">
								{formatTokens(usage.weekly.usedTokens)} / {formatTokens(usage.weekly.limitTokens)}
								{#if usage.weekly.resetAt}
									<span class="ml-1">&middot; {timeUntilReset(usage.weekly.resetAt)}</span>
								{/if}
							</span>
						</div>
						<div class="h-1.5 rounded-full bg-base-content/10 overflow-hidden">
							<div
								class="h-full rounded-full transition-all {usage.weekly.percentUsed > 80 ? 'bg-warning' : 'bg-primary'}"
								style="width: {usage.weekly.percentUsed}%"
							></div>
						</div>
					</div>
				{/if}
			</div>
		</section>

		<!-- Payment Methods -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Payment</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				{#if paymentMethods.length > 0}
					<div class="space-y-3 mb-4">
						{#each paymentMethods as pm}
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-3">
									<CreditCard class="w-5 h-5 text-base-content/80" />
									<div>
										<p class="text-base font-medium text-base-content">
											{pm.brand || pm.type} ending in {pm.lastFour || '****'}
										</p>
										{#if pm.expiresAt}
											<p class="text-sm text-base-content/80">Expires {pm.expiresAt}</p>
										{/if}
									</div>
								</div>
								{#if pm.isDefault}
									<span class="text-sm text-primary font-medium">Default</span>
								{/if}
							</div>
						{/each}
					</div>
				{:else}
					<p class="text-base text-base-content/80 mb-4">No payment method on file</p>
				{/if}
				<button
					disabled={actionLoading === 'portal'}
					onclick={handlePortal}
					class="h-9 px-4 rounded-xl border border-base-content/10 text-base font-medium text-base-content hover:bg-base-content/5 transition-colors flex items-center gap-1.5"
				>
					{#if actionLoading === 'portal'}
						<Spinner size={14} />
						<span>Opening...</span>
					{:else}
						Manage payment
						<ExternalLink class="w-3.5 h-3.5 text-base-content/80" />
					{/if}
				</button>
				<p class="text-sm text-base-content/80 mt-2">Opens Stripe in your browser</p>
			</div>
		</section>

		<!-- Invoices -->
		{#if invoices.length > 0}
			<section>
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Invoices</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/5">
					{#each invoices.slice(0, 5) as inv}
						<div class="flex items-center justify-between p-4">
							<div class="flex items-center gap-3">
								<Receipt class="w-4 h-4 text-base-content/80" />
								<div>
									<p class="text-base text-base-content">{inv.description || 'Invoice'}</p>
									<p class="text-sm text-base-content/80">{formatDate(inv.createdAt)}</p>
								</div>
							</div>
							<div class="flex items-center gap-3">
								<span class="text-base font-medium text-base-content tabular-nums">
									{formatPrice(inv.amountCents, inv.currency)}
								</span>
								{#if inv.pdfUrl}
									<a
										href={inv.pdfUrl}
										target="_blank"
										rel="noopener noreferrer"
										class="text-base text-primary hover:brightness-110 transition-all flex items-center gap-1"
									>
										PDF
										<ExternalLink class="w-3 h-3" />
									</a>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		<!-- Give Nebo -->
		<section>
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">Give Nebo</h3>
				<button
					type="button"
					onclick={() => (showGiftInfo = true)}
					class="flex items-center gap-1 text-sm text-base-content/50 hover:text-base-content/80 transition-colors"
				>
					<Info class="w-3.5 h-3.5" />
					<span>How it works</span>
				</button>
			</div>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-center gap-3 mb-1">
					<Gift class="w-5 h-5 text-primary" />
					<p class="text-base font-medium text-base-content">Give a friend a bonus 1M tokens</p>
				</div>
				<p class="text-sm text-base-content/60 mb-4 ml-8">They get 3M tokens on signup plus a bonus 1M from you — 4M total to start. You get 3M when they try it.</p>
				{#if referralCode}
					<div class="flex flex-col gap-2">
						<div class="flex items-center gap-2">
							<span class="flex-1 font-mono text-base font-bold tracking-widest bg-base-300/60 rounded-xl px-4 py-2.5 text-center text-base-content">
								{referralCode}
							</span>
							<button
								type="button"
								onclick={copyReferralCode}
								class="h-10 w-10 rounded-xl bg-base-300/60 hover:bg-base-content/10 flex items-center justify-center transition-colors shrink-0"
								title="Copy code"
							>
								{#if referralCopied}
									<Check class="w-4 h-4 text-success" />
								{:else}
									<Copy class="w-4 h-4 text-base-content/60" />
								{/if}
							</button>
						</div>
						<button
							type="button"
							onclick={copyReferralLink}
							class="flex items-center justify-between gap-2 w-full text-left text-base text-base-content/60 hover:text-base-content bg-base-300/40 hover:bg-base-300/60 rounded-xl px-4 py-2.5 transition-colors"
						>
							<span class="truncate">{referralLink}</span>
							{#if referralLinkCopied}
								<Check class="w-3.5 h-3.5 text-success shrink-0" />
							{:else}
								<Copy class="w-3.5 h-3.5 shrink-0" />
							{/if}
						</button>
					</div>
				{:else}
					<div class="flex items-center gap-2">
						<Spinner size={14} />
						<span class="text-base text-base-content/60">Loading your gift link...</span>
					</div>
				{/if}
			</div>
		</section>

		<!-- Cancel / Delete Account -->
		<section>
			{#if currentPlan !== 'free' && subscription?.subscriptions?.length}
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Cancellation</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-center justify-between">
						<p class="text-base text-base-content/80">Cancel your {planName} plan</p>
						<button
							disabled={actionLoading !== ''}
							onclick={() => handleCancel(subscription!.subscriptions[0].id)}
							class="text-base font-medium text-error hover:brightness-110 transition-colors"
						>
							{#if actionLoading === 'cancel'}
								<Spinner size={14} />
							{:else}
								Cancel plan
							{/if}
						</button>
					</div>
				</div>
			{:else}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="flex items-center justify-between">
						<p class="text-base text-base-content/80">Want to remove your account and all data?</p>
						<button
							onclick={openDeleteModal}
							class="text-base font-medium text-error hover:brightness-110 transition-colors"
						>
							Delete account
						</button>
					</div>
				</div>
			{/if}
		</section>
	</div>
{/if}

<!-- Delete Account Confirmation Modal -->
<Modal bind:show={showDeleteModal} title="Delete Account" size="sm">
	<div class="space-y-4">
		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<div class="flex gap-3">
				<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
				<div>
					<p class="text-base font-medium text-error">This action is permanent</p>
					<p class="text-base text-error/80 mt-1">
						Your account, settings, memories, and all associated data will be permanently deleted. This cannot be undone.
					</p>
				</div>
			</div>
		</div>

		<div>
			<label class="block text-base font-medium text-base-content mb-1" for="confirm-delete">
				Type <code class="bg-base-200 px-1.5 py-0.5 rounded text-error font-bold">DELETE</code> to confirm
			</label>
			<input
				id="confirm-delete"
				type="text"
				class="input input-bordered w-full text-base"
				placeholder="Type DELETE to confirm"
				bind:value={deleteConfirmText}
				onkeydown={(e) => {
					if (e.key === 'Enter' && canDelete) handleDeleteAccount();
				}}
			/>
		</div>
	</div>

	{#snippet footer()}
		<button
			onclick={() => (showDeleteModal = false)}
			class="h-9 px-4 rounded-xl border border-base-content/10 text-base font-medium text-base-content hover:bg-base-content/5 transition-colors"
		>
			Cancel
		</button>
		<button
			disabled={!canDelete || deleteLoading}
			onclick={handleDeleteAccount}
			class="h-9 px-4 rounded-xl text-base font-bold transition-all flex items-center gap-2
				{canDelete ? 'bg-error text-error-content hover:brightness-110' : 'bg-base-content/10 text-base-content/60 cursor-not-allowed'}"
		>
			{#if deleteLoading}
				<Spinner size={14} />
			{/if}
			Delete my account
		</button>
	{/snippet}
</Modal>

<!-- How Gift Works Modal -->
<Modal bind:show={showGiftInfo} title="How Giving Nebo Works" size="sm">
	<div class="space-y-5">
		<div class="space-y-4">
			<div class="flex gap-3">
				<div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
					<span class="text-sm font-bold text-primary">1</span>
				</div>
				<div>
					<p class="text-base font-medium text-base-content">Share your link</p>
					<p class="text-sm text-base-content/60">Send your personal link to someone you want to have Nebo.</p>
				</div>
			</div>
			<div class="flex gap-3">
				<div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
					<span class="text-sm font-bold text-primary">2</span>
				</div>
				<div>
					<p class="text-base font-medium text-base-content">They start with 4M tokens</p>
					<p class="text-sm text-base-content/60">Everyone gets 3M on signup. Your gift adds a bonus 1M — so they start with 4 million tokens.</p>
				</div>
			</div>
			<div class="flex gap-3">
				<div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
					<span class="text-sm font-bold text-primary">3</span>
				</div>
				<div>
					<p class="text-base font-medium text-base-content">You get 3M tokens</p>
					<p class="text-sm text-base-content/60">Once they try Nebo, you receive 3 million tokens as a thank you.</p>
				</div>
			</div>
		</div>

		<div class="rounded-xl bg-base-200/50 border border-base-content/10 p-4">
			<p class="text-sm font-medium text-base-content mb-2">Gift Milestones</p>
			<div class="space-y-1.5">
				{#each [
					{ count: 3, tier: 'Guide', reward: '+50M tokens' },
					{ count: 5, tier: 'Builder', reward: '+100M tokens' },
					{ count: 10, tier: 'Pathfinder', reward: '+250M tokens' },
					{ count: 25, tier: 'Benefactor', reward: '+500M tokens' },
					{ count: 50, tier: 'Patron', reward: '+1B tokens' },
					{ count: 100, tier: "Founder's Circle", reward: '+2B tokens' }
				] as milestone}
					<div class="flex items-center justify-between text-sm">
						<span class="text-base-content/80">{milestone.count} gifts &rarr; <span class="font-medium text-base-content">{milestone.tier}</span></span>
						<span class="text-primary font-medium tabular-nums">{milestone.reward}</span>
					</div>
				{/each}
			</div>
		</div>

		<p class="text-sm text-base-content/50">
			The more people you bring along, the more tokens you earn. Each milestone unlocks additional perks on your NeboLoop profile. All bonus tokens expire 90 days after they're granted.
			<a href="https://getnebo.com/legal/gifting-terms" target="_blank" rel="noopener noreferrer" class="text-primary hover:brightness-110 transition-all">Gifting Terms</a>
		</p>
	</div>
</Modal>
