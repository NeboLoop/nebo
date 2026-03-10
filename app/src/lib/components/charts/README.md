# Ballast Charts Library

Professional chart components built with pure SVG - no external charting libraries required.

## Components

### Chart Components

- **BarChart** - Vertical bar charts with multi-series support
- **LineChart** - Line charts with smooth bezier curves and area fill
- **PieChart** - Pie and donut charts with percentage labels
- **ScatterChart** - Scatter plots with optional trendlines
- **HeatMap** - Heat map visualizations with color gradients
- **TableChart** - Sortable data tables with pagination
- **MetricCard** - KPI cards with trend indicators

### Wrapper Components

- **ChartCard** - Universal wrapper with loading, error states, export, and fullscreen

### Layout Components

- **GridLayout** - Responsive grid system with drag-and-drop (in analytics folder)
- **MetricsRow** - Row of metric cards with responsive columns (in analytics folder)

## Implementation

All charts are built with:
- **Pure SVG** - No Chart.js or other charting libraries
- **Svelte 5 runes** - `$state`, `$derived`, `$props` for reactivity
- **ResizeObserver** - Automatic responsive sizing
- **Zero dependencies** - Just Svelte

## Usage Examples

### Bar Chart

```svelte
<script>
	import { BarChart, ChartCard } from '$lib/components/charts';

	const data = {
		labels: ['Q1', 'Q2', 'Q3', 'Q4'],
		datasets: [
			{
				label: 'Revenue',
				data: [45000, 52000, 48000, 61000],
				backgroundColor: 'rgba(99, 102, 241, 0.8)'
			}
		]
	};
</script>

<ChartCard title="Quarterly Revenue" subtitle="2024 Performance">
	<BarChart {data} showLegend={true} showGrid={true} />
</ChartCard>
```

### Line Chart

```svelte
<script>
	import { LineChart, ChartCard } from '$lib/components/charts';

	const data = {
		labels: ['Jan', 'Feb', 'Mar', 'Apr', 'May'],
		datasets: [
			{
				label: 'Users',
				data: [1200, 1450, 1650, 1800, 2100],
				borderColor: 'rgba(99, 102, 241, 1)'
			},
			{
				label: 'Sessions',
				data: [3200, 3850, 4200, 4600, 5100],
				borderColor: 'rgba(168, 85, 247, 1)'
			}
		]
	};
</script>

<ChartCard title="User Growth">
	<LineChart {data} smooth={true} fill={true} showDots={true} />
</ChartCard>
```

### Metric Cards with Trends

```svelte
<script>
	import { MetricCard } from '$lib/components/charts';
	import { MetricsRow } from '$lib/components/analytics';
	import { DollarSign, Users, TrendingUp, Activity } from 'lucide-svelte';

	const metrics = [
		{
			title: 'Total Revenue',
			value: 57103.56,
			trend: 12.5,
			trendPeriod: 'vs last month',
			format: 'currency',
			icon: DollarSign,
			color: 'green'
		},
		{
			title: 'Active Users',
			value: 2847,
			trend: -3.2,
			trendPeriod: 'vs last week',
			format: 'number',
			icon: Users,
			color: 'indigo'
		},
		{
			title: 'Conversion Rate',
			value: 3.24,
			trend: 0.8,
			trendPeriod: 'vs last month',
			format: 'percentage',
			icon: TrendingUp,
			color: 'purple'
		},
		{
			title: 'Avg Session',
			value: 245,
			trend: 15.3,
			trendPeriod: 'vs last week',
			format: 'duration',
			icon: Activity,
			color: 'orange'
		}
	];
</script>

<MetricsRow {metrics} columns={4} animated={true} />
```

### Pie/Donut Chart

```svelte
<script>
	import { PieChart, ChartCard } from '$lib/components/charts';

	const data = {
		labels: ['Direct', 'Organic', 'Social', 'Referral', 'Email'],
		datasets: [
			{
				data: [4200, 3100, 2400, 1800, 1200]
			}
		]
	};
</script>

<ChartCard title="Traffic Sources">
	<PieChart {data} type="doughnut" showLegend={true} />
</ChartCard>
```

### Scatter Chart with Trendline

```svelte
<script>
	import { ScatterChart, ChartCard } from '$lib/components/charts';

	const data = [
		{
			label: 'Dataset 1',
			data: [
				{ x: 10, y: 20 },
				{ x: 20, y: 35 },
				{ x: 30, y: 45 },
				{ x: 40, y: 60 }
			]
		}
	];

	const config = {
		xLabel: 'Experience (years)',
		yLabel: 'Salary ($K)'
	};
</script>

<ChartCard title="Salary vs Experience">
	<ScatterChart {data} {config} showTrendline={true} />
</ChartCard>
```

### Heat Map

```svelte
<script>
	import { HeatMap, ChartCard } from '$lib/components/charts';

	const data = [
		{ x: 'Mon', y: '9 AM', value: 45 },
		{ x: 'Mon', y: '10 AM', value: 72 },
		{ x: 'Tue', y: '9 AM', value: 38 },
		{ x: 'Tue', y: '10 AM', value: 65 }
		// ... more data
	];
</script>

<ChartCard title="Activity Heatmap">
	<HeatMap {data} colorScheme="indigo" showValues={true} />
</ChartCard>
```

### Table Chart

```svelte
<script>
	import { TableChart, ChartCard } from '$lib/components/charts';

	const columns = [
		{ key: 'name', label: 'Name', sortable: true },
		{ key: 'revenue', label: 'Revenue', sortable: true, align: 'right',
		  format: (val) => `$${val.toLocaleString()}` },
		{ key: 'growth', label: 'Growth', sortable: true, align: 'right',
		  format: (val) => `${val}%` }
	];

	const rows = [
		{ name: 'Product A', revenue: 45000, growth: 12.5 },
		{ name: 'Product B', revenue: 38000, growth: -3.2 },
		{ name: 'Product C', revenue: 52000, growth: 18.7 }
	];
</script>

<ChartCard title="Product Performance">
	<TableChart {columns} {rows} paginate={true} pageSize={10} />
</ChartCard>
```

### Grid Layout with Drag & Drop

```svelte
<script>
	import { GridLayout } from '$lib/components/analytics';
	import { ChartCard, BarChart, LineChart } from '$lib/components/charts';

	let editMode = false;
	let items = [
		{ id: 'chart1', x: 0, y: 0, w: 6, h: 3 },
		{ id: 'chart2', x: 6, y: 0, w: 6, h: 3 },
		{ id: 'chart3', x: 0, y: 3, w: 12, h: 4 }
	];

	function handleLayoutChange(newItems) {
		items = newItems;
		// Save to backend
	}
</script>

<button on:click={() => editMode = !editMode}>
	{editMode ? 'Save Layout' : 'Edit Layout'}
</button>

<GridLayout
	bind:items
	{editMode}
	columns={12}
	rowHeight={100}
	gap={16}
	onLayoutChange={handleLayoutChange}
>
	{#snippet children(item)}
		{#if item.id === 'chart1'}
			<ChartCard title="Revenue">
				<BarChart data={revenueData} />
			</ChartCard>
		{:else if item.id === 'chart2'}
			<ChartCard title="Users">
				<LineChart data={userData} />
			</ChartCard>
		{:else}
			<ChartCard title="Performance">
				<!-- Other chart -->
			</ChartCard>
		{/if}
	{/snippet}
</GridLayout>
```

## Chart Configuration

All charts accept a `config` prop with these common options:

```typescript
interface ChartConfig {
	title?: string;           // Chart title
	subtitle?: string;        // Chart subtitle
	description?: string;     // Chart description
	xLabel?: string;          // X-axis label
	yLabel?: string;          // Y-axis label
	showLegend?: boolean;     // Show/hide legend (default: true)
	showGrid?: boolean;       // Show/hide grid lines (default: true)
	showTooltips?: boolean;   // Show/hide tooltips (default: true)
	responsive?: boolean;     // Responsive sizing (default: true)
	maintainAspectRatio?: boolean; // Maintain aspect ratio (default: true)
	aspectRatio?: number;     // Custom aspect ratio (default varies by chart)
}
```

## ChartCard Features

The `ChartCard` wrapper provides:

- **Loading State**: Skeleton loader while data is fetching
- **Error State**: Error message with retry button
- **Refresh**: Manual refresh button
- **Export**: Export as PNG, CSV, or JSON
- **Fullscreen**: View chart in fullscreen mode
- **Last Updated**: Timestamp of last data update

## Styling

All charts use the Ballast color scheme:
- Indigo/Purple gradients for primary elements
- Dark theme compatible
- Smooth animations and transitions
- Accessible color contrasts
- Responsive design

## Performance

- Charts use `ResizeObserver` with cleanup on unmount
- Uses Svelte 5 `$derived` for efficient reactive updates
- CSS transitions for smooth hover effects
- Lightweight - no heavy charting library overhead

## Accessibility

- Proper ARIA labels on all interactive elements
- Keyboard navigation support
- Screen reader friendly
- High contrast colors for readability
