<script lang="ts">
	import { onMount } from 'svelte';

	interface GanttTask {
		id: string;
		label: string;
		start: Date | string;
		end: Date | string;
		progress?: number; // 0-100
		color?: string;
		dependencies?: string[];
		status?: 'pending' | 'in-progress' | 'completed' | 'blocked';
	}

	interface GanttData {
		tasks: GanttTask[];
	}

	interface Props {
		data?: GanttData;
		tasks?: GanttTask[];
		height?: number;
		showProgress?: boolean;
		showDependencies?: boolean;
		rowHeight?: number;
	}

	let {
		data,
		tasks: propTasks = [],
		height,
		showProgress = true,
		showDependencies = true,
		rowHeight = 40
	}: Props = $props();

	// Extract tasks from data object or use direct prop
	const tasks = $derived(data?.tasks ?? propTasks);

	let containerRef: HTMLDivElement;
	let containerWidth = $state(600);

	onMount(() => {
		if (containerRef) {
			const observer = new ResizeObserver((entries) => {
				for (const entry of entries) {
					containerWidth = Math.max(entry.contentRect.width, 300);
				}
			});
			observer.observe(containerRef);
			return () => observer.disconnect();
		}
	});

	const labelWidth = 150;
	const chartWidth = $derived(containerWidth - labelWidth);
	const effectiveHeight = $derived(height || Math.max(tasks.length * rowHeight + 60, 200));

	const statusColors = {
		'pending': 'rgba(148, 163, 184, 0.7)',
		'in-progress': 'rgba(99, 102, 241, 0.85)',
		'completed': 'rgba(34, 197, 94, 0.85)',
		'blocked': 'rgba(239, 68, 68, 0.85)'
	};

	const defaultColors = [
		'rgba(99, 102, 241, 0.85)',
		'rgba(168, 85, 247, 0.85)',
		'rgba(236, 72, 153, 0.85)',
		'rgba(251, 146, 60, 0.85)',
		'rgba(34, 197, 94, 0.85)',
	];

	// Parse dates and calculate time range
	const parsedTasks = $derived.by(() => {
		return tasks.map((task, index) => {
			const start = task.start instanceof Date ? task.start : new Date(task.start);
			const end = task.end instanceof Date ? task.end : new Date(task.end);
			return {
				...task,
				startDate: start,
				endDate: end,
				index
			};
		});
	});

	const timeRange = $derived.by(() => {
		if (parsedTasks.length === 0) {
			const now = new Date();
			return { start: now, end: new Date(now.getTime() + 30 * 24 * 60 * 60 * 1000) };
		}

		let minDate = parsedTasks[0].startDate;
		let maxDate = parsedTasks[0].endDate;

		for (const task of parsedTasks) {
			if (task.startDate < minDate) minDate = task.startDate;
			if (task.endDate > maxDate) maxDate = task.endDate;
		}

		// Add some padding
		const padding = (maxDate.getTime() - minDate.getTime()) * 0.05;
		return {
			start: new Date(minDate.getTime() - padding),
			end: new Date(maxDate.getTime() + padding)
		};
	});

	const totalDuration = $derived(timeRange.end.getTime() - timeRange.start.getTime());

	function dateToX(date: Date): number {
		return ((date.getTime() - timeRange.start.getTime()) / totalDuration) * chartWidth;
	}

	function getBarWidth(start: Date, end: Date): number {
		return Math.max(((end.getTime() - start.getTime()) / totalDuration) * chartWidth, 4);
	}

	function getColor(task: GanttTask, index: number): string {
		if (task.status) return statusColors[task.status];
		return task.color || defaultColors[index % defaultColors.length];
	}

	function formatDate(date: Date): string {
		return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
	}

	// Generate time axis ticks
	const timeTicks = $derived.by(() => {
		const ticks: { date: Date; x: number; label: string }[] = [];
		const duration = totalDuration;
		const dayMs = 24 * 60 * 60 * 1000;

		let interval: number;
		if (duration <= 7 * dayMs) {
			interval = dayMs; // Daily
		} else if (duration <= 30 * dayMs) {
			interval = 7 * dayMs; // Weekly
		} else if (duration <= 180 * dayMs) {
			interval = 30 * dayMs; // Monthly
		} else {
			interval = 90 * dayMs; // Quarterly
		}

		let current = new Date(timeRange.start);
		current.setHours(0, 0, 0, 0);

		while (current <= timeRange.end) {
			if (current >= timeRange.start) {
				ticks.push({
					date: new Date(current),
					x: dateToX(current),
					label: formatDate(current)
				});
			}
			current = new Date(current.getTime() + interval);
		}

		return ticks;
	});

	// Find task by ID for dependency drawing
	function findTaskIndex(id: string): number {
		return parsedTasks.findIndex(t => t.id === id);
	}
</script>

<div bind:this={containerRef} class="gantt-chart-container w-full overflow-x-auto">
	{#if tasks.length === 0}
		<div class="empty-state flex items-center justify-center" style="height: {effectiveHeight}px;">
			<p class="text-base-content/60 text-sm">No tasks to display</p>
		</div>
	{:else}
		<svg width={containerWidth} height={effectiveHeight}>
			<!-- Header / Time axis -->
			<g transform="translate({labelWidth}, 0)">
				<rect x="0" y="0" width={chartWidth} height="40" fill="rgba(30, 41, 59, 0.5)" />
				{#each timeTicks as tick}
					<line
						x1={tick.x}
						y1="40"
						x2={tick.x}
						y2={effectiveHeight}
						stroke="rgba(51, 65, 85, 0.3)"
						stroke-width="1"
						stroke-dasharray="4,4"
					/>
					<text
						x={tick.x}
						y="25"
						text-anchor="middle"
						class="text-xs fill-base-content/60"
					>
						{tick.label}
					</text>
				{/each}
			</g>

			<!-- Task labels -->
			<g>
				{#each parsedTasks as task, i}
					<text
						x="10"
						y={50 + i * rowHeight + rowHeight / 2}
						dominant-baseline="middle"
						class="text-sm fill-base-content/70"
					>
						{task.label.length > 18 ? task.label.slice(0, 18) + '..' : task.label}
					</text>
				{/each}
			</g>

			<!-- Dependencies -->
			{#if showDependencies}
				<g transform="translate({labelWidth}, 50)">
					{#each parsedTasks as task}
						{#if task.dependencies}
							{#each task.dependencies as depId}
								{@const depIndex = findTaskIndex(depId)}
								{#if depIndex >= 0}
									{@const depTask = parsedTasks[depIndex]}
									{@const startX = dateToX(depTask.endDate)}
									{@const startY = depIndex * rowHeight + rowHeight / 2}
									{@const endX = dateToX(task.startDate)}
									{@const endY = task.index * rowHeight + rowHeight / 2}
									<path
										d="M {startX} {startY} C {startX + 20} {startY}, {endX - 20} {endY}, {endX} {endY}"
										fill="none"
										stroke="rgba(148, 163, 184, 0.4)"
										stroke-width="2"
										marker-end="url(#arrowhead)"
									/>
								{/if}
							{/each}
						{/if}
					{/each}
				</g>
			{/if}

			<!-- Task bars -->
			<g transform="translate({labelWidth}, 50)">
				{#each parsedTasks as task, i}
					{@const barX = dateToX(task.startDate)}
					{@const barWidth = getBarWidth(task.startDate, task.endDate)}
					{@const barY = i * rowHeight + (rowHeight - 24) / 2}

					<!-- Background bar -->
					<rect
						x={barX}
						y={barY}
						width={barWidth}
						height="24"
						fill={getColor(task, i)}
						rx="4"
						class="transition-all duration-200 hover:opacity-80 cursor-pointer"
					>
						<title>{task.label}: {formatDate(task.startDate)} - {formatDate(task.endDate)}{task.progress !== undefined ? ` (${task.progress}%)` : ''}</title>
					</rect>

					<!-- Progress bar -->
					{#if showProgress && task.progress !== undefined && task.progress > 0}
						<rect
							x={barX}
							y={barY}
							width={barWidth * (task.progress / 100)}
							height="24"
							fill="rgba(255, 255, 255, 0.2)"
							rx="4"
							class="pointer-events-none"
						/>
					{/if}

					<!-- Task label on bar if wide enough -->
					{#if barWidth > 60}
						<text
							x={barX + barWidth / 2}
							y={barY + 12}
							text-anchor="middle"
							dominant-baseline="middle"
							class="text-xs fill-white font-medium pointer-events-none"
						>
							{task.progress !== undefined ? `${task.progress}%` : ''}
						</text>
					{/if}
				{/each}
			</g>

			<!-- Arrow marker definition -->
			<defs>
				<marker
					id="arrowhead"
					markerWidth="10"
					markerHeight="7"
					refX="9"
					refY="3.5"
					orient="auto"
				>
					<polygon points="0 0, 10 3.5, 0 7" fill="rgba(148, 163, 184, 0.4)" />
				</marker>
			</defs>
		</svg>

		<!-- Legend -->
		<div class="legend mt-2">
			<div class="legend-item">
				<span class="legend-dot" style="background-color: {statusColors['pending']}"></span>
				<span class="legend-label">Pending</span>
			</div>
			<div class="legend-item">
				<span class="legend-dot" style="background-color: {statusColors['in-progress']}"></span>
				<span class="legend-label">In Progress</span>
			</div>
			<div class="legend-item">
				<span class="legend-dot" style="background-color: {statusColors['completed']}"></span>
				<span class="legend-label">Completed</span>
			</div>
			<div class="legend-item">
				<span class="legend-dot" style="background-color: {statusColors['blocked']}"></span>
				<span class="legend-label">Blocked</span>
			</div>
		</div>
	{/if}
</div>
