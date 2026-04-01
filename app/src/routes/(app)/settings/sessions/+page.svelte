<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { ChevronLeft, ChevronRight, Trash2, MessageSquare, AlertTriangle, RefreshCw } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { AgentSession } from '$lib/api/nebo';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { t } from 'svelte-i18n';

	let sessions = $state<AgentSession[]>([]);
	let isLoading = $state(true);

	// Calendar state
	let viewYear = $state(new Date().getFullYear());
	let viewMonth = $state(new Date().getMonth()); // 0-indexed
	let selectedDate = $state<string | null>(null); // "YYYY-MM-DD"

	// Delete state
	let showDeleteModal = $state(false);
	let deleteTarget = $state<{ type: 'single'; session: AgentSession } | { type: 'day'; date: string } | { type: 'older'; days: number } | null>(null);
	let deleteConfirmText = $state('');
	let isDeleting = $state(false);

	// Bulk cleanup
	let showCleanupMenu = $state(false);

	const WEEKDAY_KEYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'] as const;
	const MONTH_KEYS = ['january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september', 'october', 'november', 'december'] as const;
	const WEEKDAYS = $derived(WEEKDAY_KEYS.map(k => $t(`weekdays.${k}`)));
	const MONTHS = $derived(MONTH_KEYS.map(k => $t(`months.${k}`)));

	onMount(async () => {
		await loadSessions();
	});

	async function loadSessions() {
		isLoading = true;
		try {
			const data = await api.listAgentSessions();
			sessions = data.sessions || [];
		} catch (error) {
			console.error('Failed to load sessions:', error);
		} finally {
			isLoading = false;
		}
	}

	// --- Date helpers ---

	function parseSessionDate(dateStr: string): Date {
		const ts = Number(dateStr);
		// If it's a unix timestamp in seconds (< year 2100 in ms = ~4.1e12)
		if (!isNaN(ts) && ts > 0 && ts < 4e12) {
			return new Date(ts * 1000);
		}
		return new Date(dateStr);
	}

	function toDateKey(d: Date): string {
		return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	}

	function formatTime(dateStr: string): string {
		const d = parseSessionDate(dateStr);
		return d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
	}

	function formatFullDate(dateKey: string): string {
		const [y, m, d] = dateKey.split('-').map(Number);
		const date = new Date(y, m - 1, d);
		return date.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' });
	}

	function formatRelative(dateStr: string): string {
		const d = parseSessionDate(dateStr);
		const now = new Date();
		const diffMs = now.getTime() - d.getTime();
		const diffMins = Math.floor(diffMs / 60000);
		if (diffMins < 1) return $t('time.justNow');
		if (diffMins < 60) return $t('time.minutesAgo', { values: { n: diffMins } });
		const diffHrs = Math.floor(diffMins / 60);
		if (diffHrs < 24) return $t('time.hoursAgo', { values: { n: diffHrs } });
		const diffDays = Math.floor(diffHrs / 24);
		if (diffDays < 7) return $t('time.daysAgo', { values: { n: diffDays } });
		return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
	}

	// --- Session grouping ---

	/** Map of "YYYY-MM-DD" → sessions for that day */
	let sessionsByDate = $derived.by(() => {
		const map = new Map<string, AgentSession[]>();
		for (const s of sessions) {
			const d = parseSessionDate(s.updatedAt || s.createdAt);
			const key = toDateKey(d);
			if (!map.has(key)) map.set(key, []);
			map.get(key)!.push(s);
		}
		// Sort each day's sessions by most recent first
		for (const [, arr] of map) {
			arr.sort((a, b) => {
				const da = parseSessionDate(b.updatedAt || b.createdAt).getTime();
				const db = parseSessionDate(a.updatedAt || a.createdAt).getTime();
				return da - db;
			});
		}
		return map;
	});

	/** Sessions for the selected date */
	let selectedSessions = $derived(
		selectedDate ? (sessionsByDate.get(selectedDate) || []) : []
	);

	/** Parse session source from name prefix */
	function sessionSource(s: AgentSession): { type: string; label: string } {
		const name = s.name || s.id;
		if (name.startsWith('agent:')) return { type: 'agent', label: $t('settingsSessions.sources.agent') };
		if (name.startsWith('channel:')) return { type: 'channel', label: $t('settingsSessions.sources.channel') };
		if (name.startsWith('heartbeat')) return { type: 'heartbeat', label: $t('settingsSessions.sources.heartbeat') };
		if (name.startsWith('workflow:')) return { type: 'workflow', label: $t('settingsSessions.sources.workflow') };
		return { type: 'chat', label: $t('settingsSessions.sources.chat') };
	}

	function sessionDisplayName(s: AgentSession): string {
		const name = s.name || s.id;
		// Strip source prefixes for display
		const prefixes = ['agent:', 'channel:', 'workflow:', 'heartbeat:'];
		for (const prefix of prefixes) {
			if (name.startsWith(prefix)) {
				const rest = name.slice(prefix.length).trim();
				return rest || prefix.slice(0, -1);
			}
		}
		// Truncate UUIDs
		if (/^[0-9a-f]{8}-[0-9a-f]{4}/.test(name)) {
			return name.slice(0, 8) + '...';
		}
		return name;
	}

	const sourceColors: Record<string, string> = {
		chat: 'text-primary bg-primary/10',
		agent: 'text-info bg-info/10',
		channel: 'text-success bg-success/10',
		heartbeat: 'text-warning bg-warning/10',
		workflow: 'text-secondary bg-secondary/10',
	};

	// --- Session navigation ---

	async function navigateToSession(session: AgentSession) {
		const name = session.name || session.id;

		if (name.startsWith('agent:')) {
			// agent:<agentId>:<channel> → navigate to that agent's activity tab
			const parts = name.split(':');
			const agentId = parts[1];
			goto(`/agent/persona/${agentId}/activity?session=${session.id}`);
			return;
		}

		if (name.startsWith('channel:')) {
			const channelName = name.slice('channel:'.length);
			goto(`/agent/channel/${encodeURIComponent(channelName)}`);
			return;
		}

		// heartbeat, workflow, companion chat → assistant's activity tab
		goto(`/agent/assistant/activity?session=${session.id}`);
	}

	// --- Calendar math ---

	let calendarDays = $derived.by(() => {
		const firstDay = new Date(viewYear, viewMonth, 1).getDay();
		const daysInMonth = new Date(viewYear, viewMonth + 1, 0).getDate();
		const daysInPrevMonth = new Date(viewYear, viewMonth, 0).getDate();

		const days: { day: number; current: boolean; dateKey: string }[] = [];

		// Previous month padding
		for (let i = firstDay - 1; i >= 0; i--) {
			const d = daysInPrevMonth - i;
			const m = viewMonth === 0 ? 12 : viewMonth;
			const y = viewMonth === 0 ? viewYear - 1 : viewYear;
			days.push({ day: d, current: false, dateKey: `${y}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}` });
		}

		// Current month
		for (let d = 1; d <= daysInMonth; d++) {
			days.push({ day: d, current: true, dateKey: `${viewYear}-${String(viewMonth + 1).padStart(2, '0')}-${String(d).padStart(2, '0')}` });
		}

		// Next month padding to fill grid
		const remaining = 42 - days.length; // 6 rows × 7
		for (let d = 1; d <= remaining; d++) {
			const m = viewMonth === 11 ? 1 : viewMonth + 2;
			const y = viewMonth === 11 ? viewYear + 1 : viewYear;
			days.push({ day: d, current: false, dateKey: `${y}-${String(m).padStart(2, '0')}-${String(d).padStart(2, '0')}` });
		}

		return days;
	});

	let todayKey = $derived(toDateKey(new Date()));

	function prevMonth() {
		if (viewMonth === 0) { viewYear--; viewMonth = 11; }
		else { viewMonth--; }
		selectedDate = null;
	}

	function nextMonth() {
		if (viewMonth === 11) { viewYear++; viewMonth = 0; }
		else { viewMonth++; }
		selectedDate = null;
	}

	function goToToday() {
		const now = new Date();
		viewYear = now.getFullYear();
		viewMonth = now.getMonth();
		selectedDate = todayKey;
	}

	function selectDay(dateKey: string) {
		selectedDate = selectedDate === dateKey ? null : dateKey;
	}

	// --- Stats ---

	let totalSessions = $derived(sessions.length);
	let totalMessages = $derived(sessions.reduce((sum, s) => sum + (s.messageCount || 0), 0));

	// --- Delete ---

	function confirmDeleteSession(session: AgentSession) {
		deleteTarget = { type: 'single', session };
		deleteConfirmText = '';
		showDeleteModal = true;
	}

	function confirmDeleteDay(date: string) {
		deleteTarget = { type: 'day', date };
		deleteConfirmText = '';
		showDeleteModal = true;
	}

	function confirmDeleteOlder(days: number) {
		deleteTarget = { type: 'older', days };
		deleteConfirmText = '';
		showDeleteModal = true;
		showCleanupMenu = false;
	}

	let deleteLabel = $derived.by(() => {
		if (!deleteTarget) return '';
		if (deleteTarget.type === 'single') return `session "${sessionDisplayName(deleteTarget.session)}"`;
		if (deleteTarget.type === 'day') {
			const count = sessionsByDate.get(deleteTarget.date)?.length || 0;
			return `${count} session${count !== 1 ? 's' : ''} from ${formatFullDate(deleteTarget.date)}`;
		}
		if (deleteTarget.type === 'older') {
			const cutoff = new Date();
			cutoff.setDate(cutoff.getDate() - deleteTarget.days);
			const count = sessions.filter(s => parseSessionDate(s.updatedAt || s.createdAt) < cutoff).length;
			return `${count} session${count !== 1 ? 's' : ''} older than ${deleteTarget.days} days`;
		}
		return '';
	});

	let canDelete = $derived(deleteConfirmText === 'DELETE');

	async function executeDelete() {
		if (!canDelete || !deleteTarget) return;
		isDeleting = true;
		try {
			if (deleteTarget.type === 'single') {
				await api.deleteAgentSession(deleteTarget.session.id);
				sessions = sessions.filter(s => s.id !== deleteTarget!.session.id);
			} else if (deleteTarget.type === 'day') {
				const daySessions = sessionsByDate.get(deleteTarget.date) || [];
				for (const s of daySessions) {
					await api.deleteAgentSession(s.id);
				}
				sessions = sessions.filter(s => {
					const key = toDateKey(parseSessionDate(s.updatedAt || s.createdAt));
					return key !== deleteTarget!.date;
				});
			} else if (deleteTarget.type === 'older') {
				const cutoff = new Date();
				cutoff.setDate(cutoff.getDate() - deleteTarget.days);
				const old = sessions.filter(s => parseSessionDate(s.updatedAt || s.createdAt) < cutoff);
				for (const s of old) {
					await api.deleteAgentSession(s.id);
				}
				sessions = sessions.filter(s => parseSessionDate(s.updatedAt || s.createdAt) >= cutoff);
			}
		} catch (err) {
			console.error('Failed to delete:', err);
		} finally {
			isDeleting = false;
			showDeleteModal = false;
			deleteTarget = null;
			deleteConfirmText = '';
		}
	}
</script>

<!-- Header -->
<div class="mb-6">
	<div class="flex items-center justify-between mb-1">
		<h2 class="font-display text-xl font-bold text-base-content">{$t('settingsSessions.title')}</h2>
		<div class="flex items-center gap-2">
			<!-- Cleanup dropdown -->
			<div class="relative">
				<button
					type="button"
					class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
					onclick={() => showCleanupMenu = !showCleanupMenu}
				>
					<Trash2 class="w-3.5 h-3.5" />
					{$t('settingsSessions.cleanup')}
				</button>
				{#if showCleanupMenu}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div class="absolute right-0 top-full mt-1 w-52 rounded-xl bg-base-100 border border-base-content/10 shadow-lg z-10 py-1"
						onmouseleave={() => showCleanupMenu = false}
					>
						<button type="button" class="w-full px-3 py-2 text-left text-base text-base-content/80 hover:bg-base-content/5 transition-colors" onclick={() => confirmDeleteOlder(30)}>
							{$t('settingsSessions.olderThan30')}
						</button>
						<button type="button" class="w-full px-3 py-2 text-left text-base text-base-content/80 hover:bg-base-content/5 transition-colors" onclick={() => confirmDeleteOlder(90)}>
							{$t('settingsSessions.olderThan90')}
						</button>
						<button type="button" class="w-full px-3 py-2 text-left text-base text-base-content/80 hover:bg-base-content/5 transition-colors" onclick={() => confirmDeleteOlder(180)}>
							{$t('settingsSessions.olderThan6Months')}
						</button>
					</div>
				{/if}
			</div>
			<button
				type="button"
				class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
				onclick={loadSessions}
			>
				<RefreshCw class="w-3.5 h-3.5" />
			</button>
		</div>
	</div>
	<p class="text-base text-base-content/80">
		{$t('settingsSessions.stats', { values: { sessions: totalSessions.toLocaleString(), messages: totalMessages.toLocaleString() } })}
	</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsSessions.loadingSessions')}</span>
	</div>
{:else}
	<!-- Calendar -->
	<section class="mb-5">
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-4">
			<!-- Month nav -->
			<div class="flex items-center justify-between mb-3">
				<button type="button" class="p-1.5 rounded-lg hover:bg-base-content/10 transition-colors" onclick={prevMonth}>
					<ChevronLeft class="w-4 h-4 text-base-content/90" />
				</button>
				<button type="button" class="text-base font-semibold text-base-content hover:text-primary transition-colors" onclick={goToToday}>
					{MONTHS[viewMonth]} {viewYear}
				</button>
				<button type="button" class="p-1.5 rounded-lg hover:bg-base-content/10 transition-colors" onclick={nextMonth}>
					<ChevronRight class="w-4 h-4 text-base-content/90" />
				</button>
			</div>

			<!-- Weekday headers -->
			<div class="grid grid-cols-7 mb-1">
				{#each WEEKDAYS as day}
					<div class="text-center text-sm font-medium text-base-content/60 py-1">{day}</div>
				{/each}
			</div>

			<!-- Day grid -->
			<div class="grid grid-cols-7">
				{#each calendarDays as { day, current, dateKey }}
					{@const count = sessionsByDate.get(dateKey)?.length || 0}
					{@const isToday = dateKey === todayKey}
					{@const isSelected = dateKey === selectedDate}
					<button
						type="button"
						class="relative flex flex-col items-center justify-center py-1.5 rounded-lg transition-colors
							{!current ? 'text-base-content/40' : ''}
							{isSelected ? 'bg-primary/15 text-primary ring-1 ring-primary/30' : ''}
							{isToday && !isSelected ? 'font-bold text-primary' : ''}
							{current && !isSelected && !isToday ? 'text-base-content/90 hover:bg-base-content/5' : ''}
							{count > 0 && current && !isSelected ? 'text-base-content' : ''}"
						onclick={() => selectDay(dateKey)}
					>
						<span class="text-sm leading-none">{day}</span>
						{#if count > 0}
							<div class="flex gap-0.5 mt-1">
								{#if count <= 3}
									{#each Array(count) as _}
										<div class="w-1 h-1 rounded-full {isSelected ? 'bg-primary' : 'bg-base-content/40'}"></div>
									{/each}
								{:else}
									<div class="w-1 h-1 rounded-full {isSelected ? 'bg-primary' : 'bg-base-content/40'}"></div>
									<div class="w-1 h-1 rounded-full {isSelected ? 'bg-primary' : 'bg-base-content/40'}"></div>
									<span class="text-[9px] leading-none {isSelected ? 'text-primary' : 'text-base-content/60'}">{count}</span>
								{/if}
							</div>
						{:else}
							<div class="h-[6px] mt-1"></div>
						{/if}
					</button>
				{/each}
			</div>
		</div>
	</section>

	<!-- Selected day timeline -->
	{#if selectedDate}
		<section>
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">
					{formatFullDate(selectedDate)}
				</h3>
				{#if selectedSessions.length > 1}
					<button
						type="button"
						class="text-sm text-base-content/80 hover:text-error transition-colors"
						onclick={() => confirmDeleteDay(selectedDate!)}
					>
						{$t('settingsSessions.deleteDay')}
					</button>
				{/if}
			</div>

			{#if selectedSessions.length === 0}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-8 text-center">
					<p class="text-base text-base-content/80">{$t('settingsSessions.noSessions')}</p>
				</div>
			{:else}
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10">
					{#each selectedSessions as session}
						{@const source = sessionSource(session)}
						<button
							type="button"
							class="w-full flex items-center gap-3 px-4 py-3 text-left hover:bg-base-content/5 transition-colors cursor-pointer"
							onclick={() => navigateToSession(session)}
						>
							<!-- Source badge -->
							<span class="text-sm font-semibold uppercase tracking-wider px-1.5 py-0.5 rounded {sourceColors[source.type] || 'text-base-content/80 bg-base-content/5'}">
								{source.label}
							</span>
							<!-- Name + meta -->
							<div class="flex-1 min-w-0">
								<p class="text-base font-medium text-base-content truncate">{sessionDisplayName(session)}</p>
								<p class="text-sm text-base-content/80">
									{formatTime(session.updatedAt || session.createdAt)}
									&middot; {$t('settingsSessions.messageCount', { values: { count: session.messageCount } })}
								</p>
							</div>
							<!-- Delete -->
							<span
								role="button"
								tabindex="0"
								class="p-1.5 rounded-lg text-base-content/60 hover:text-error hover:bg-error/10 transition-colors shrink-0"
								onclick={(e) => { e.stopPropagation(); confirmDeleteSession(session); }}
								onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); confirmDeleteSession(session); } }}
							>
								<Trash2 class="w-3.5 h-3.5" />
							</span>
						</button>
					{/each}
				</div>
			{/if}
		</section>
	{:else}
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-8 text-center">
			<MessageSquare class="w-8 h-8 mx-auto mb-2 text-base-content/40" />
			<p class="text-base text-base-content/80">{$t('settingsSessions.selectDay')}</p>
		</div>
	{/if}
{/if}

<!-- Delete confirmation modal -->
<Modal
	bind:show={showDeleteModal}
	title={$t('settingsSessions.deleteTitle')}
	size="md"
	closeOnBackdrop={false}
	showCloseButton={false}
	onclose={() => { showDeleteModal = false; deleteTarget = null; deleteConfirmText = ''; }}
>
	<div class="space-y-4">
		<div class="flex items-start gap-3">
			<AlertTriangle class="w-5 h-5 text-error shrink-0 mt-0.5" />
			<div class="text-base text-base-content">
				<p>{$t('settingsSessions.deleteDescription', { values: { label: deleteLabel } })}</p>
			</div>
		</div>

		<div class="rounded-xl bg-error/10 border border-error/20 p-4">
			<p class="text-base font-semibold text-error mb-1">{$t('settingsSessions.permanentMemoryLoss')}</p>
			<p class="text-base text-base-content/80">
				{$t('settingsSessions.deleteExplanation')}
			</p>
		</div>

		<div>
			<label class="block text-base font-medium text-base-content mb-1" for="confirm-delete-sessions">
				{$t('settingsSessions.typeDeleteConfirm')}
			</label>
			<input
				id="confirm-delete-sessions"
				type="text"
				class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
				placeholder={$t('settingsSessions.typeDeleteConfirm')}
				bind:value={deleteConfirmText}
				onkeydown={(e) => { if (e.key === 'Enter' && canDelete) executeDelete(); }}
			/>
		</div>
	</div>

	{#snippet footer()}
		<div class="flex justify-end gap-2 w-full">
			<button
				type="button"
				class="h-9 px-4 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
				onclick={() => { showDeleteModal = false; deleteTarget = null; deleteConfirmText = ''; }}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="button"
				class="h-9 px-4 rounded-full bg-error text-white text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
				onclick={executeDelete}
				disabled={!canDelete || isDeleting}
			>
				{#if isDeleting}{$t('settingsSessions.deleting')}{:else}{$t('settingsSessions.deletePermanently')}{/if}
			</button>
		</div>
	{/snippet}
</Modal>
