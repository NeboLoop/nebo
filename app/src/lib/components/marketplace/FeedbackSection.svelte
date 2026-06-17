<script lang="ts">
	import { Bug, Lightbulb, HelpCircle } from 'lucide-svelte';
	import { getStoreProductFeedback, submitStoreProductFeedback } from '$lib/api/nebo';

	interface FeedbackItem {
		id: string;
		targetId: string;
		targetType: string;
		userId: string;
		userName?: string;
		user_name?: string;
		type: string;
		title: string;
		body?: string;
		status: string;
		createdAt?: string;
		created_at?: string;
	}

	let {
		feedback = [],
		targetId,
		targetType
	}: {
		feedback: FeedbackItem[];
		targetId: string;
		targetType: string;
	} = $props();

	let showForm = $state(false);
	let fbType = $state<'bug' | 'feature' | 'question'>('bug');
	let fbTitle = $state('');
	let fbBody = $state('');
	let submitting = $state(false);
	let items = $state<FeedbackItem[]>([]);

	$effect(() => {
		items = feedback;
	});

	const typeIcons: Record<string, typeof Bug> = {
		bug: Bug,
		feature: Lightbulb,
		question: HelpCircle
	};

	const typeLabels: Record<string, string> = {
		bug: 'Bug Report',
		feature: 'Feature Request',
		question: 'Question'
	};

	const statusColors: Record<string, string> = {
		open: 'badge-warning',
		acknowledged: 'badge-info',
		resolved: 'badge-success',
		closed: 'badge-neutral'
	};

	function timeAgo(dateStr: string) {
		if (!dateStr) return '';
		const diff = Date.now() - new Date(dateStr).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 60) return `${mins}m ago`;
		const hours = Math.floor(mins / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		if (days < 30) return `${days}d ago`;
		return `${Math.floor(days / 30)}mo ago`;
	}

	async function submitFeedback() {
		if (!fbTitle.trim()) return;
		submitting = true;
		try {
			await submitStoreProductFeedback(targetId, { type: fbType, title: fbTitle, body: fbBody });
			showForm = false;
			fbTitle = '';
			fbBody = '';
			const res = await getStoreProductFeedback(targetId);
			items = (res as { feedback?: FeedbackItem[] }).feedback ?? [];
		} catch { /* ignore */ }
		submitting = false;
	}
</script>

<section class="flex flex-col gap-3">
	<div class="flex items-center justify-between">
		<h2 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Feedback</h2>
		<button type="button" onclick={() => showForm = true} class="text-sm text-primary font-medium hover:underline">Submit feedback</button>
	</div>

	{#if items.length > 0}
		<div class="flex flex-col gap-2">
			{#each items as fb}
				{@const FbIcon = typeIcons[fb.type] ?? Bug}
				<div class="rounded-xl bg-base-100 border border-base-300 p-3.5">
					<div class="flex items-start gap-3">
						<div class="w-7 h-7 rounded-lg bg-base-200 flex items-center justify-center shrink-0 mt-0.5">
							<FbIcon class="w-3.5 h-3.5 text-base-content/50" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2 mb-1">
								<span class="text-sm font-semibold truncate">{fb.title}</span>
								<span class="badge badge-xs {statusColors[fb.status] ?? 'badge-neutral'}">{fb.status}</span>
							</div>
							{#if fb.body}
								<p class="text-sm text-base-content/60 line-clamp-2">{fb.body}</p>
							{/if}
							<div class="flex items-center gap-2 mt-1.5 text-xs text-base-content/50">
								<span>{fb.userName ?? fb.user_name ?? 'User'}</span>
								<span>·</span>
								<span>{timeAgo(fb.createdAt ?? fb.created_at ?? '')}</span>
							</div>
						</div>
					</div>
				</div>
			{/each}
		</div>
	{:else}
		<div class="rounded-2xl bg-base-100 border border-base-300 py-8 text-center">
			<p class="text-sm text-base-content/50">No feedback yet</p>
		</div>
	{/if}
</section>

{#if showForm}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm" onclick={() => (showForm = false)} aria-label="Close feedback form"></button>
		<div class="relative bg-base-100 rounded-2xl border border-base-300 w-full max-w-md mx-4 p-6">
			<h3 class="font-display text-lg font-bold mb-4">Submit feedback</h3>
			<div class="grid grid-cols-3 gap-2 mb-3">
				{#each (['bug', 'feature', 'question'] as const) as t}
					{@const Icon = typeIcons[t]}
					<button type="button" onclick={() => (fbType = t)} class="flex flex-col items-center gap-1 px-2 py-2.5 rounded-xl text-xs font-medium transition-colors border {fbType === t ? 'bg-primary/10 text-primary border-primary/30' : 'bg-base-200 text-base-content/60 border-transparent hover:bg-base-300'}">
						<Icon class="w-4 h-4" />
						{typeLabels[t]}
					</button>
				{/each}
			</div>
			<input type="text" bind:value={fbTitle} placeholder="Title" class="input input-bordered w-full mb-2" />
			<textarea bind:value={fbBody} rows="4" placeholder="Description (optional)" class="textarea textarea-bordered w-full resize-none"></textarea>
			<div class="flex gap-2 mt-4">
				<button type="button" onclick={() => (showForm = false)} class="btn btn-ghost flex-1">Cancel</button>
				<button type="button" onclick={submitFeedback} disabled={!fbTitle.trim() || submitting} class="btn btn-primary flex-1 disabled:opacity-40">
					{submitting ? 'Submitting…' : 'Submit'}
				</button>
			</div>
		</div>
	</div>
{/if}
