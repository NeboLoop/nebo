<!--
  Time Picker Component
  Standalone time input with hours, minutes, and optional AM/PM
-->

<script lang="ts">
	import { Clock, ChevronUp, ChevronDown } from 'lucide-svelte';

	interface Props {
		value?: string;
		label?: string;
		size?: 'sm' | 'md' | 'lg';
		use24Hour?: boolean;
		step?: number;
		minTime?: string;
		maxTime?: string;
		disabled?: boolean;
		required?: boolean;
		class?: string;
		onchange?: (value: string) => void;
	}

	let {
		value = $bindable('12:00'),
		label,
		size = 'md',
		use24Hour = false,
		step = 1,
		minTime,
		maxTime,
		disabled = false,
		required = false,
		class: extraClass = '',
		onchange
	}: Props = $props();

	// Parse value into hours, minutes, and period
	let hours = $state(12);
	let minutes = $state(0);
	let period = $state<'AM' | 'PM'>('AM');

	// Initialize from value
	$effect(() => {
		if (value) {
			const [h, m] = value.split(':').map(Number);
			if (use24Hour) {
				hours = h;
			} else {
				hours = h === 0 ? 12 : h > 12 ? h - 12 : h;
				period = h >= 12 ? 'PM' : 'AM';
			}
			minutes = m || 0;
		}
	});

	// Update value when components change
	function updateValue() {
		let h = hours;
		if (!use24Hour) {
			if (period === 'AM' && h === 12) h = 0;
			else if (period === 'PM' && h !== 12) h += 12;
		}
		const newValue = `${h.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}`;
		value = newValue;
		onchange?.(newValue);
	}

	function incrementHours() {
		const max = use24Hour ? 23 : 12;
		const min = use24Hour ? 0 : 1;
		hours = hours >= max ? min : hours + 1;
		updateValue();
	}

	function decrementHours() {
		const max = use24Hour ? 23 : 12;
		const min = use24Hour ? 0 : 1;
		hours = hours <= min ? max : hours - 1;
		updateValue();
	}

	function incrementMinutes() {
		minutes = minutes >= 59 ? 0 : minutes + step;
		if (minutes > 59) minutes = 0;
		updateValue();
	}

	function decrementMinutes() {
		minutes = minutes <= 0 ? 60 - step : minutes - step;
		if (minutes < 0) minutes = 59;
		updateValue();
	}

	function togglePeriod() {
		period = period === 'AM' ? 'PM' : 'AM';
		updateValue();
	}

	function handleHoursInput(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		let val = parseInt(target.value) || 0;
		const max = use24Hour ? 23 : 12;
		const min = use24Hour ? 0 : 1;
		hours = Math.max(min, Math.min(max, val));
		updateValue();
	}

	function handleMinutesInput(e: Event) {
		const target = e.currentTarget as HTMLInputElement;
		let val = parseInt(target.value) || 0;
		minutes = Math.max(0, Math.min(59, val));
		updateValue();
	}

	const sizeClasses: Record<string, { wrapper: string; input: string; btn: string }> = {
		sm: { wrapper: 'gap-1', input: 'w-10 h-8 text-sm', btn: 'w-6 h-6' },
		md: { wrapper: 'gap-2', input: 'w-12 h-10', btn: 'w-8 h-8' },
		lg: { wrapper: 'gap-2', input: 'w-14 h-12 text-lg', btn: 'w-10 h-10' }
	};

	const sizes = $derived(sizeClasses[size]);
</script>

<div class="w-fit {extraClass}">
	{#if label}
		<label class="label">
			<span class="label-text font-medium">
				{label}
				{#if required}
					<span class="text-error">*</span>
				{/if}
			</span>
		</label>
	{/if}

	<div
		class="flex items-center {sizes.wrapper} p-2 bg-base-100 border border-base-300 rounded-lg"
		class:opacity-50={disabled}
	>
		<Clock class="w-4 h-4 text-base-content/40" />

		<!-- Hours -->
		<div class="flex flex-col items-center">
			<button
				type="button"
				class="btn btn-ghost btn-xs {sizes.btn} p-0"
				onclick={incrementHours}
				{disabled}
				tabindex="-1"
			>
				<ChevronUp class="w-4 h-4" />
			</button>
			<input
				type="text"
				class="input input-bordered text-center font-mono {sizes.input} p-0"
				value={hours.toString().padStart(2, '0')}
				oninput={handleHoursInput}
				{disabled}
				maxlength="2"
			/>
			<button
				type="button"
				class="btn btn-ghost btn-xs {sizes.btn} p-0"
				onclick={decrementHours}
				{disabled}
				tabindex="-1"
			>
				<ChevronDown class="w-4 h-4" />
			</button>
		</div>

		<span class="text-xl font-bold text-base-content/60">:</span>

		<!-- Minutes -->
		<div class="flex flex-col items-center">
			<button
				type="button"
				class="btn btn-ghost btn-xs {sizes.btn} p-0"
				onclick={incrementMinutes}
				{disabled}
				tabindex="-1"
			>
				<ChevronUp class="w-4 h-4" />
			</button>
			<input
				type="text"
				class="input input-bordered text-center font-mono {sizes.input} p-0"
				value={minutes.toString().padStart(2, '0')}
				oninput={handleMinutesInput}
				{disabled}
				maxlength="2"
			/>
			<button
				type="button"
				class="btn btn-ghost btn-xs {sizes.btn} p-0"
				onclick={decrementMinutes}
				{disabled}
				tabindex="-1"
			>
				<ChevronDown class="w-4 h-4" />
			</button>
		</div>

		<!-- AM/PM Toggle -->
		{#if !use24Hour}
			<button
				type="button"
				class="btn btn-ghost btn-sm font-semibold min-w-12"
				onclick={togglePeriod}
				{disabled}
			>
				{period}
			</button>
		{/if}
	</div>
</div>
