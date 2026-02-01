<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Clock, Plus, Trash2, Play, Power, RefreshCw, X, Save, History, AlertCircle, CheckCircle } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { TaskItem, TaskHistoryItem } from '$lib/api/nebo';

	let tasks = $state<TaskItem[]>([]);
	let isLoading = $state(true);
	let selectedTask = $state<TaskItem | null>(null);
	let taskHistory = $state<TaskHistoryItem[]>([]);
	let showCreateModal = $state(false);
	let isRunning = $state(false);

	// Create form state
	let newTask = $state({
		name: '',
		schedule: '0 9 * * *',
		taskType: 'message',
		command: '',
		message: '',
		deliver: '',
		enabled: true
	});

	// Common schedule presets
	const schedulePresets = [
		{ label: 'Every hour', value: '0 * * * *' },
		{ label: 'Every morning at 9am', value: '0 9 * * *' },
		{ label: 'Every evening at 6pm', value: '0 18 * * *' },
		{ label: 'Every day at noon', value: '0 12 * * *' },
		{ label: 'Every Monday at 9am', value: '0 9 * * 1' },
		{ label: 'Every weekday at 9am', value: '0 9 * * 1-5' }
	];

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
		if (!newTask.name || !newTask.schedule) {
			alert('Name and schedule are required');
			return;
		}

		try {
			await api.createTask(newTask);
			showCreateModal = false;
			resetNewTask();
			await loadTasks();
		} catch (error) {
			console.error('Failed to create task:', error);
		}
	}

	function resetNewTask() {
		newTask = {
			name: '',
			schedule: '0 9 * * *',
			taskType: 'message',
			command: '',
			message: '',
			deliver: '',
			enabled: true
		};
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
		if (!confirm(`Delete task "${task.name}"?`)) return;

		try {
			await api.deleteTask(String(task.id));
			tasks = tasks.filter(t => t.id !== task.id);
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
		const preset = schedulePresets.find(p => p.value === cron);
		if (preset) return preset.label;

		// Simple cron to human readable
		const parts = cron.split(' ');
		if (parts.length !== 5) return cron;

		const [min, hour, dom, month, dow] = parts;

		if (hour !== '*' && min === '0' && dom === '*' && month === '*') {
			if (dow === '*') return `Daily at ${hour}:00`;
			if (dow === '1-5') return `Weekdays at ${hour}:00`;
			if (dow === '1') return `Mondays at ${hour}:00`;
		}

		if (min === '0' && hour === '*') return 'Every hour';

		return cron;
	}

	function getTaskTypeLabel(type: string): string {
		switch (type) {
			case 'message': return 'Send Message';
			case 'bash': return 'Run Command';
			default: return type;
		}
	}
</script>

<svelte:head>
	<title>Tasks - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Tasks</h1>
		<p class="text-sm text-base-content/60">Scheduled and automated tasks</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={loadTasks}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={() => showCreateModal = true}>
			<Plus class="w-4 h-4 mr-2" />
			New Task
		</Button>
	</div>
</div>

<div class="grid lg:grid-cols-2 gap-6">
	<!-- Task List -->
	<div>
		<Card>
			<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
				<Clock class="w-5 h-5" />
				All Tasks
			</h2>
			{#if isLoading}
				<div class="py-8 text-center text-base-content/60">Loading...</div>
			{:else if tasks.length === 0}
				<div class="py-8 text-center text-base-content/60">
					<Clock class="w-8 h-8 mx-auto mb-2 opacity-50" />
					<p>No tasks yet</p>
					<p class="text-xs mt-1">Create a task to automate actions</p>
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
								<span class="badge badge-ghost badge-xs">{getTaskTypeLabel(task.taskType)}</span>
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

	<!-- Task Detail & History -->
	<div>
		<Card class="h-[calc(100vh-220px)]">
			{#if selectedTask}
				<h2 class="font-display font-bold text-base-content mb-4">{selectedTask.name}</h2>

				<!-- Task Info -->
				<div class="mb-6 space-y-2 text-sm">
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Schedule:</span>
						<span class="font-mono text-xs bg-base-200 px-2 py-1 rounded">{selectedTask.schedule}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Type:</span>
						<span>{getTaskTypeLabel(selectedTask.taskType)}</span>
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
							<span class="text-base-content/60 w-24">Deliver via:</span>
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
					<div class="py-4 text-center text-base-content/60 text-sm">
						No execution history yet
					</div>
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
						<Clock class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
						<p class="text-base-content/60">Select a task to view details</p>
					</div>
				</div>
			{/if}
		</Card>
	</div>
</div>

<!-- Create Task Modal -->
{#if showCreateModal}
	<div class="modal modal-open">
		<div class="modal-box">
			<h3 class="font-bold text-lg mb-4">Create New Task</h3>

			<div class="space-y-4">
				<div>
					<label class="label" for="task-name">
						<span class="label-text">Task Name</span>
					</label>
					<input
						id="task-name"
						type="text"
						class="input input-bordered w-full"
						placeholder="e.g., Morning Briefing"
						bind:value={newTask.name}
					/>
				</div>

				<div>
					<label class="label" for="task-type">
						<span class="label-text">Task Type</span>
					</label>
					<select id="task-type" class="select select-bordered w-full" bind:value={newTask.taskType}>
						<option value="message">Send Message</option>
						<option value="bash">Run Command</option>
					</select>
				</div>

				<div>
					<label class="label" for="task-schedule">
						<span class="label-text">Schedule</span>
					</label>
					<select id="task-schedule" class="select select-bordered w-full" bind:value={newTask.schedule}>
						{#each schedulePresets as preset}
							<option value={preset.value}>{preset.label}</option>
						{/each}
					</select>
					<p class="text-xs text-base-content/50 mt-1">Cron: {newTask.schedule}</p>
				</div>

				{#if newTask.taskType === 'message'}
					<div>
						<label class="label" for="task-message">
							<span class="label-text">Message</span>
						</label>
						<textarea
							id="task-message"
							class="textarea textarea-bordered w-full"
							placeholder="What should the agent do?"
							rows="3"
							bind:value={newTask.message}
						></textarea>
					</div>

					<div>
						<label class="label" for="task-deliver">
							<span class="label-text">Deliver Via (optional)</span>
						</label>
						<select id="task-deliver" class="select select-bordered w-full" bind:value={newTask.deliver}>
							<option value="">None (internal only)</option>
							<option value="telegram">Telegram</option>
							<option value="discord">Discord</option>
							<option value="slack">Slack</option>
						</select>
					</div>
				{:else}
					<div>
						<label class="label" for="task-command">
							<span class="label-text">Command</span>
						</label>
						<input
							id="task-command"
							type="text"
							class="input input-bordered w-full font-mono text-sm"
							placeholder="e.g., echo 'Hello'"
							bind:value={newTask.command}
						/>
					</div>
				{/if}

				<div class="form-control">
					<label class="label cursor-pointer justify-start gap-3">
						<input type="checkbox" class="checkbox checkbox-primary" bind:checked={newTask.enabled} />
						<span class="label-text">Enable immediately</span>
					</label>
				</div>
			</div>

			<div class="modal-action">
				<Button type="ghost" onclick={() => { showCreateModal = false; resetNewTask(); }}>
					Cancel
				</Button>
				<Button type="primary" onclick={createTask}>
					<Save class="w-4 h-4 mr-2" />
					Create Task
				</Button>
			</div>
		</div>
		<div class="modal-backdrop" onclick={() => { showCreateModal = false; resetNewTask(); }}></div>
	</div>
{/if}
