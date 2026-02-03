<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Clock, Plus, Trash2, Play, Power, RefreshCw, X, Save, History, AlertCircle, CheckCircle, CalendarClock } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { TaskItem, TaskHistoryItem } from '$lib/api/nebo';

	let tasks = $state<TaskItem[]>([]);
	let isLoading = $state(true);
	let selectedTask = $state<TaskItem | null>(null);
	let taskHistory = $state<TaskHistoryItem[]>([]);
	let showCreateModal = $state(false);
	let isRunning = $state(false);

	// --- Scheduler state ---
	type Frequency = 'hourly' | 'daily' | 'weekly' | 'monthly' | 'custom';

	let taskName = $state('');
	let frequency = $state<Frequency>('daily');
	let hour = $state(9);
	let minute = $state(0);
	let hourlyInterval = $state(1);
	let weekDays = $state<boolean[]>([false, true, true, true, true, true, false]); // Mon-Fri default
	let monthDay = $state(1);
	let customCron = $state('');
	let taskMessage = $state('');
	let taskDeliver = $state('');
	let taskEnabled = $state(true);

	const dayLabels = ['S', 'M', 'T', 'W', 'T', 'F', 'S'];

	let cronExpression = $derived.by(() => {
		switch (frequency) {
			case 'hourly':
				return `0 */${hourlyInterval} * * *`;
			case 'daily':
				return `${minute} ${hour} * * *`;
			case 'weekly': {
				const days = weekDays
					.map((on, i) => (on ? i : -1))
					.filter((d) => d >= 0)
					.join(',');
				return `${minute} ${hour} * * ${days || '1'}`;
			}
			case 'monthly':
				return `${minute} ${hour} ${monthDay} * *`;
			case 'custom':
				return customCron;
		}
	});

	let cronPreview = $derived(formatSchedule(cronExpression));

	onMount(async () => {
		await loadTasks();
	});

	async function loadTasks() {
		isLoading = true;
		try {
			const data = await api.listTasks({ page: 1, pageSize: 100 });
			tasks = data.tasks || [];
		} catch (error) {
			console.error('Failed to load tasks:', error);
		} finally {
			isLoading = false;
		}
	}

	async function selectTask(task: TaskItem) {
		selectedTask = task;
		await loadHistory(task.id);
	}

	async function loadHistory(taskId: number) {
		try {
			const data = await api.listTaskHistory({ page: 1, pageSize: 50 }, String(taskId));
			taskHistory = data.history || [];
		} catch (error) {
			console.error('Failed to load history:', error);
		}
	}

	async function createTask() {
		if (!taskName || !cronExpression) {
			alert('Name and schedule are required for an event');
			return;
		}

		try {
			await api.createTask({
				name: taskName,
				schedule: cronExpression,
				taskType: 'message',
				message: taskMessage,
				deliver: taskDeliver || undefined,
				enabled: taskEnabled
			});
			showCreateModal = false;
			resetForm();
			await loadTasks();
		} catch (error) {
			console.error('Failed to create task:', error);
		}
	}

	function resetForm() {
		taskName = '';
		frequency = 'daily';
		hour = 9;
		minute = 0;
		hourlyInterval = 1;
		weekDays = [false, true, true, true, true, true, false];
		monthDay = 1;
		customCron = '';
		taskMessage = '';
		taskDeliver = '';
		taskEnabled = true;
	}

	async function toggleTask(task: TaskItem, e: Event) {
		e.stopPropagation();
		try {
			const data = await api.toggleTask(String(task.id));
			task.enabled = data.enabled;
			tasks = [...tasks];
		} catch (error) {
			console.error('Failed to toggle task:', error);
		}
	}

	async function runTask(task: TaskItem, e: Event) {
		e.stopPropagation();
		isRunning = true;
		try {
			await api.runTask(String(task.id));
			await loadTasks();
			if (selectedTask?.id === task.id) {
				await loadHistory(task.id);
			}
		} catch (error) {
			console.error('Failed to run task:', error);
		} finally {
			isRunning = false;
		}
	}

	async function deleteTask(task: TaskItem, e: Event) {
		e.stopPropagation();
		if (!confirm(`Delete event "${task.name}"?`)) return;

		try {
			await api.deleteTask(String(task.id));
			tasks = tasks.filter((t) => t.id !== task.id);
			if (selectedTask?.id === task.id) {
				selectedTask = null;
				taskHistory = [];
			}
		} catch (error) {
			console.error('Failed to delete task:', error);
		}
	}

	function formatDate(dateStr: string): string {
		if (!dateStr) return 'Never';
		return new Date(dateStr).toLocaleString();
	}

	function formatSchedule(cron: string): string {
		if (!cron) return '';
		const parts = cron.split(' ');
		if (parts.length !== 5) return cron;

		const [min, hr, dom, month, dow] = parts;

		// Hourly patterns
		if (hr.startsWith('*/')) {
			const interval = parseInt(hr.replace('*/', ''));
			if (interval === 1) return 'Every hour';
			return `Every ${interval} hours`;
		}
		if (hr === '*' && min === '0') return 'Every hour';

		const pad = (n: string) => n.padStart(2, '0');
		const timeStr = `${pad(hr)}:${pad(min)}`;

		// Monthly
		if (dom !== '*' && month === '*' && dow === '*') {
			const d = parseInt(dom);
			const suffix = d === 1 || d === 21 || d === 31 ? 'st' : d === 2 || d === 22 ? 'nd' : d === 3 || d === 23 ? 'rd' : 'th';
			return `Monthly on the ${d}${suffix} at ${timeStr}`;
		}

		// Weekly
		if (dom === '*' && month === '*' && dow !== '*') {
			const dayNames = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
			if (dow === '1-5') return `Weekdays at ${timeStr}`;
			if (dow === '0,6') return `Weekends at ${timeStr}`;
			const dayList = dow.split(',').map((d) => dayNames[parseInt(d)] || d);
			if (dayList.length === 1) return `${dayList[0]}s at ${timeStr}`;
			return `${dayList.join(', ')} at ${timeStr}`;
		}

		// Daily
		if (dom === '*' && month === '*' && dow === '*' && hr !== '*') {
			return `Daily at ${timeStr}`;
		}

		return cron;
	}

	function getFrequencyBadge(cron: string): string {
		if (!cron) return '';
		const parts = cron.split(' ');
		if (parts.length !== 5) return 'custom';

		const [, hr, dom, , dow] = parts;
		if (hr.startsWith('*/') || hr === '*') return 'hourly';
		if (dom !== '*') return 'monthly';
		if (dow !== '*') return 'weekly';
		return 'daily';
	}

	function isValidCron(expr: string): boolean {
		if (!expr) return false;
		const parts = expr.trim().split(/\s+/);
		if (parts.length !== 5) return false;
		const cronRegex = /^[\d,\-\*\/]+$/;
		return parts.every((p) => cronRegex.test(p));
	}
</script>

<svelte:head>
	<title>Schedule - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Schedule</h1>
		<p class="text-sm text-base-content/60">Scheduled automations for your agent</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={loadTasks}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={() => (showCreateModal = true)}>
			<Plus class="w-4 h-4 mr-2" />
			New Event
		</Button>
	</div>
</div>

<div class="grid lg:grid-cols-2 gap-6">
	<!-- Event List -->
	<div>
		<Card>
			<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
				<CalendarClock class="w-5 h-5" />
				Events
			</h2>
			{#if isLoading}
				<div class="py-8 text-center text-base-content/60">Loading...</div>
			{:else if tasks.length === 0}
				<div class="py-8 text-center text-base-content/60">
					<CalendarClock class="w-8 h-8 mx-auto mb-2 opacity-50" />
					<p>No events yet</p>
					<p class="text-xs mt-1">Create an event to automate actions</p>
				</div>
			{:else}
				<div class="space-y-3">
					{#each tasks as task}
						<div
							class="p-4 rounded-lg transition-colors cursor-pointer {selectedTask?.id === task.id ? 'bg-primary/10 border border-primary/30' : 'bg-base-200 hover:bg-base-300'}"
							onclick={() => selectTask(task)}
							onkeydown={(e) => e.key === 'Enter' && selectTask(task)}
							role="button"
							tabindex="0"
						>
							<div class="flex items-start justify-between gap-2 mb-2">
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="font-medium">{task.name}</span>
										{#if task.enabled}
											<span class="badge badge-success badge-sm">Active</span>
										{:else}
											<span class="badge badge-ghost badge-sm">Disabled</span>
										{/if}
									</div>
									<p class="text-sm text-base-content/60 mt-1">{formatSchedule(task.schedule)}</p>
								</div>
								<div class="flex gap-1">
									<button
										class="btn btn-ghost btn-xs"
										onclick={(e) => toggleTask(task, e)}
										title={task.enabled ? 'Disable' : 'Enable'}
									>
										<Power class="w-4 h-4 {task.enabled ? 'text-success' : 'text-base-content/40'}" />
									</button>
									<button
										class="btn btn-ghost btn-xs"
										onclick={(e) => runTask(task, e)}
										title="Run now"
										disabled={isRunning}
									>
										<Play class="w-4 h-4" />
									</button>
									<button
										class="btn btn-ghost btn-xs text-error"
										onclick={(e) => deleteTask(task, e)}
										title="Delete"
									>
										<Trash2 class="w-4 h-4" />
									</button>
								</div>
							</div>
							<div class="flex items-center gap-4 text-xs text-base-content/50">
								<span class="badge badge-ghost badge-xs">{getFrequencyBadge(task.schedule)}</span>
								<span>Runs: {task.runCount}</span>
								{#if task.lastRun}
									<span>Last: {formatDate(task.lastRun)}</span>
								{/if}
							</div>
							{#if task.lastError}
								<div class="mt-2 text-xs text-error flex items-center gap-1">
									<AlertCircle class="w-3 h-3" />
									{task.lastError}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</Card>
	</div>

	<!-- Event Detail & History -->
	<div>
		<Card class="h-[calc(100vh-220px)]">
			{#if selectedTask}
				<h2 class="font-display font-bold text-base-content mb-4">{selectedTask.name}</h2>

				<!-- Event Info -->
				<div class="mb-6 space-y-2 text-sm">
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Schedule:</span>
						<span>{formatSchedule(selectedTask.schedule)}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Cron:</span>
						<span class="font-mono text-xs bg-base-200 px-2 py-1 rounded">{selectedTask.schedule}</span>
					</div>
					{#if selectedTask.message}
						<div class="flex items-start gap-2">
							<span class="text-base-content/60 w-24">Message:</span>
							<span class="flex-1">{selectedTask.message}</span>
						</div>
					{/if}
					{#if selectedTask.command}
						<div class="flex items-start gap-2">
							<span class="text-base-content/60 w-24">Command:</span>
							<code class="font-mono text-xs bg-base-200 px-2 py-1 rounded flex-1">{selectedTask.command}</code>
						</div>
					{/if}
					{#if selectedTask.deliver}
						<div class="flex items-center gap-2">
							<span class="text-base-content/60 w-24">Notify via:</span>
							<span>{selectedTask.deliver}</span>
						</div>
					{/if}
				</div>

				<!-- Execution History -->
				<h3 class="font-medium text-base-content mb-3 flex items-center gap-2">
					<History class="w-4 h-4" />
					Recent Executions
				</h3>
				{#if taskHistory.length === 0}
					<div class="py-4 text-center text-base-content/60 text-sm">No execution history yet</div>
				{:else}
					<div class="space-y-2 max-h-64 overflow-y-auto">
						{#each taskHistory as h}
							<div class="p-3 rounded-lg bg-base-200 text-sm">
								<div class="flex items-center gap-2 mb-1">
									{#if h.success}
										<CheckCircle class="w-4 h-4 text-success" />
										<span class="text-success font-medium">Success</span>
									{:else}
										<AlertCircle class="w-4 h-4 text-error" />
										<span class="text-error font-medium">Failed</span>
									{/if}
									<span class="text-xs text-base-content/50 ml-auto">
										{formatDate(h.startedAt)}
									</span>
								</div>
								{#if h.output}
									<p class="text-xs text-base-content/70 mt-1">{h.output}</p>
								{/if}
								{#if h.error}
									<p class="text-xs text-error mt-1">{h.error}</p>
								{/if}
							</div>
						{/each}
					</div>
				{/if}
			{:else}
				<div class="h-full flex items-center justify-center text-center">
					<div>
						<CalendarClock class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
						<p class="text-base-content/60">Select an event to view details</p>
					</div>
				</div>
			{/if}
		</Card>
	</div>
</div>

<!-- Create Event Modal -->
{#if showCreateModal}
	<div class="modal modal-open">
		<div class="modal-box max-w-lg">
			<div class="flex items-center justify-between mb-6">
				<h3 class="font-bold text-lg">New Event</h3>
				<button
					class="btn btn-ghost btn-sm btn-square"
					onclick={() => {
						showCreateModal = false;
						resetForm();
					}}
				>
					<X class="w-4 h-4" />
				</button>
			</div>

			<div class="space-y-5">
				<!-- Name -->
				<div>
					<label class="label" for="task-name">
						<span class="label-text font-medium">Name</span>
					</label>
					<input
						id="task-name"
						type="text"
						class="input input-bordered w-full"
						placeholder="Morning Briefing"
						bind:value={taskName}
					/>
				</div>

				<!-- Repeats -->
				<div>
					<label class="label" for="task-frequency">
						<span class="label-text font-medium">Repeats</span>
					</label>
					<select id="task-frequency" class="select select-bordered w-full" bind:value={frequency}>
						<option value="hourly">Every Hour</option>
						<option value="daily">Every Day</option>
						<option value="weekly">Every Week</option>
						<option value="monthly">Every Month</option>
						<option value="custom">Custom (cron)</option>
					</select>
				</div>

				<!-- Frequency-specific controls -->
				{#if frequency === 'hourly'}
					<div>
						<label class="label" for="hourly-interval">
							<span class="label-text font-medium">Interval</span>
						</label>
						<div class="flex items-center gap-3">
							<span class="text-sm text-base-content/60">Every</span>
							<input
								id="hourly-interval"
								type="number"
								class="input input-bordered w-20 text-center"
								min="1"
								max="23"
								bind:value={hourlyInterval}
							/>
							<span class="text-sm text-base-content/60">hour(s)</span>
						</div>
					</div>
				{:else if frequency === 'daily'}
					<div>
						<label class="label">
							<span class="label-text font-medium">Time</span>
						</label>
						<div class="flex items-center gap-2">
							<select class="select select-bordered w-24" bind:value={hour}>
								{#each Array.from({ length: 24 }, (_, i) => i) as h}
									<option value={h}>{String(h).padStart(2, '0')}</option>
								{/each}
							</select>
							<span class="text-lg font-bold text-base-content/40">:</span>
							<select class="select select-bordered w-24" bind:value={minute}>
								{#each Array.from({ length: 60 }, (_, i) => i) as m}
									<option value={m}>{String(m).padStart(2, '0')}</option>
								{/each}
							</select>
						</div>
					</div>
				{:else if frequency === 'weekly'}
					<div>
						<label class="label">
							<span class="label-text font-medium">Days</span>
						</label>
						<div class="flex gap-1.5">
							{#each dayLabels as label, i}
								<button
									type="button"
									class="btn btn-sm btn-square {weekDays[i] ? 'btn-primary' : 'btn-ghost bg-base-200'}"
									onclick={() => {
										weekDays[i] = !weekDays[i];
										weekDays = [...weekDays];
									}}
								>
									{label}
								</button>
							{/each}
						</div>
					</div>
					<div>
						<label class="label">
							<span class="label-text font-medium">Time</span>
						</label>
						<div class="flex items-center gap-2">
							<select class="select select-bordered w-24" bind:value={hour}>
								{#each Array.from({ length: 24 }, (_, i) => i) as h}
									<option value={h}>{String(h).padStart(2, '0')}</option>
								{/each}
							</select>
							<span class="text-lg font-bold text-base-content/40">:</span>
							<select class="select select-bordered w-24" bind:value={minute}>
								{#each Array.from({ length: 60 }, (_, i) => i) as m}
									<option value={m}>{String(m).padStart(2, '0')}</option>
								{/each}
							</select>
						</div>
					</div>
				{:else if frequency === 'monthly'}
					<div>
						<label class="label" for="month-day">
							<span class="label-text font-medium">Day of month</span>
						</label>
						<select id="month-day" class="select select-bordered w-full" bind:value={monthDay}>
							{#each Array.from({ length: 31 }, (_, i) => i + 1) as d}
								{@const suffix = d === 1 || d === 21 || d === 31 ? 'st' : d === 2 || d === 22 ? 'nd' : d === 3 || d === 23 ? 'rd' : 'th'}
								<option value={d}>{d}{suffix}</option>
							{/each}
						</select>
					</div>
					<div>
						<label class="label">
							<span class="label-text font-medium">Time</span>
						</label>
						<div class="flex items-center gap-2">
							<select class="select select-bordered w-24" bind:value={hour}>
								{#each Array.from({ length: 24 }, (_, i) => i) as h}
									<option value={h}>{String(h).padStart(2, '0')}</option>
								{/each}
							</select>
							<span class="text-lg font-bold text-base-content/40">:</span>
							<select class="select select-bordered w-24" bind:value={minute}>
								{#each Array.from({ length: 60 }, (_, i) => i) as m}
									<option value={m}>{String(m).padStart(2, '0')}</option>
								{/each}
							</select>
						</div>
					</div>
				{:else if frequency === 'custom'}
					<div>
						<label class="label" for="custom-cron">
							<span class="label-text font-medium">Cron expression</span>
						</label>
						<input
							id="custom-cron"
							type="text"
							class="input input-bordered w-full font-mono text-sm"
							placeholder="0 9 * * 1-5"
							bind:value={customCron}
						/>
						{#if customCron && !isValidCron(customCron)}
							<p class="text-xs text-error mt-1">Invalid cron expression (expected 5 fields: min hour dom month dow)</p>
						{/if}
					</div>
				{/if}

				<!-- Schedule preview -->
				{#if cronExpression && (frequency !== 'custom' || isValidCron(customCron))}
					<div class="rounded-lg bg-base-200 px-3 py-2 text-sm flex items-center gap-2">
						<Clock class="w-4 h-4 text-base-content/50 shrink-0" />
						<span class="text-base-content/70">{cronPreview}</span>
						<span class="font-mono text-xs text-base-content/40 ml-auto">{cronExpression}</span>
					</div>
				{/if}

				<!-- Message -->
				<div>
					<label class="label" for="task-message">
						<span class="label-text font-medium">What should the agent do?</span>
					</label>
					<textarea
						id="task-message"
						class="textarea textarea-bordered w-full"
						placeholder="Check my calendar for today, summarize urgent emails, and give me the weather."
						rows="3"
						bind:value={taskMessage}
					></textarea>
				</div>

				<!-- Deliver -->
				<div>
					<label class="label" for="task-deliver">
						<span class="label-text font-medium">Notify via <span class="font-normal text-base-content/50">(optional)</span></span>
					</label>
					<select id="task-deliver" class="select select-bordered w-full" bind:value={taskDeliver}>
						<option value="">None (internal only)</option>
						<option value="telegram">Telegram</option>
						<option value="discord">Discord</option>
						<option value="slack">Slack</option>
					</select>
				</div>

				<!-- Enable -->
				<div class="form-control">
					<label class="label cursor-pointer justify-start gap-3">
						<input type="checkbox" class="checkbox checkbox-primary" bind:checked={taskEnabled} />
						<span class="label-text">Enable immediately</span>
					</label>
				</div>
			</div>

			<div class="modal-action">
				<Button
					type="ghost"
					onclick={() => {
						showCreateModal = false;
						resetForm();
					}}
				>
					Cancel
				</Button>
				<Button type="primary" onclick={createTask} disabled={!taskName || !cronExpression || (frequency === 'custom' && !isValidCron(customCron))}>
					<Save class="w-4 h-4 mr-2" />
					Create
				</Button>
			</div>
		</div>
		<div
			class="modal-backdrop"
			onclick={() => {
				showCreateModal = false;
				resetForm();
			}}
		></div>
	</div>
{/if}
