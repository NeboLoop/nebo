<script lang="ts">
	import { Filter, Calendar, X } from 'lucide-svelte';

	interface DateRange {
		from: string;
		to: string;
	}

	interface Props {
		dateRange: DateRange;
		presets?: { label: string; from: string; to: string }[];
		onchange?: (range: DateRange) => void;
	}

	let { dateRange = $bindable(), presets = [], onchange }: Props = $props();

	const defaultPresets = [
		{
			label: 'Last 7 days',
			from: new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString().split('T')[0],
			to: new Date().toISOString().split('T')[0]
		},
		{
			label: 'Last 30 days',
			from: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString().split('T')[0],
			to: new Date().toISOString().split('T')[0]
		},
		{
			label: 'Last 90 days',
			from: new Date(Date.now() - 90 * 24 * 60 * 60 * 1000).toISOString().split('T')[0],
			to: new Date().toISOString().split('T')[0]
		},
		{
			label: 'This month',
			from: new Date(new Date().getFullYear(), new Date().getMonth(), 1).toISOString().split('T')[0],
			to: new Date().toISOString().split('T')[0]
		}
	];

	const allPresets = $derived(presets.length > 0 ? presets : defaultPresets);

	function handleFromChange(event: Event) {
		const input = event.target as HTMLInputElement;
		dateRange.from = input.value;
		onchange?.(dateRange);
	}

	function handleToChange(event: Event) {
		const input = event.target as HTMLInputElement;
		dateRange.to = input.value;
		onchange?.(dateRange);
	}

	function selectPreset(preset: { from: string; to: string; label: string }) {
		dateRange.from = preset.from;
		dateRange.to = preset.to;
		onchange?.(dateRange);
		// Close dropdown by removing focus
		if (document.activeElement instanceof HTMLElement) {
			document.activeElement.blur();
		}
	}

	function formatDateDisplay(from: string, to: string): string {
		const fromDate = new Date(from);
		const toDate = new Date(to);
		const options: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };

		// Check if it matches a preset
		for (const preset of allPresets) {
			if (preset.from === from && preset.to === to) {
				return preset.label;
			}
		}

		return `${fromDate.toLocaleDateString('en-US', options)} - ${toDate.toLocaleDateString('en-US', options)}`;
	}

	const displayText = $derived(formatDateDisplay(dateRange.from, dateRange.to));
</script>

<div class="dropdown dropdown-end">
	<div tabindex="0" role="button" class="btn btn-sm btn-ghost gap-2">
		<Filter size={14} class="opacity-60" />
		<span>{displayText}</span>
	</div>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div tabindex="0" class="dropdown-content z-50 w-80 bg-base-100 rounded-lg shadow-lg border border-base-300">
		<!-- Header -->
		<div class="flex items-center justify-between px-4 py-3 border-b border-base-300">
			<div class="flex items-center gap-2 text-sm font-medium">
				<Calendar size={16} />
				Date Range
			</div>
			<button
				type="button"
				onclick={() => {
					if (document.activeElement instanceof HTMLElement) {
						document.activeElement.blur();
					}
				}}
				class="btn btn-ghost btn-xs btn-circle"
			>
				<X size={16} />
			</button>
		</div>

		<!-- Presets -->
		<div class="p-3 border-b border-base-300">
			<div class="flex flex-wrap gap-2">
				{#each allPresets as preset}
					<button
						type="button"
						onclick={() => selectPreset(preset)}
						class="btn btn-xs {dateRange.from === preset.from && dateRange.to === preset.to
							? 'btn-primary'
							: 'btn-ghost'}"
					>
						{preset.label}
					</button>
				{/each}
			</div>
		</div>

		<!-- Custom Date Inputs -->
		<div class="p-4 space-y-3">
			<div class="grid grid-cols-2 gap-3">
				<div>
					<label for="from-date" class="label pb-1">
						<span class="label-text text-xs">From</span>
					</label>
					<input
						type="date"
						id="from-date"
						bind:value={dateRange.from}
						onchange={handleFromChange}
						class="input input-bordered input-sm w-full"
					/>
				</div>
				<div>
					<label for="to-date" class="label pb-1">
						<span class="label-text text-xs">To</span>
					</label>
					<input
						type="date"
						id="to-date"
						bind:value={dateRange.to}
						onchange={handleToChange}
						class="input input-bordered input-sm w-full"
					/>
				</div>
			</div>
		</div>
	</div>
</div>
