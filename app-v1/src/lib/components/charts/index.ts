/**
 * Ballast Charts Library
 * Professional analytics chart components - Pure SVG, zero dependencies
 */

// ============================================================================
// Core Chart Components
// ============================================================================
export { default as BarChart } from './BarChart.svelte';
export { default as LineChart } from './LineChart.svelte';
export { default as AreaChart } from './AreaChart.svelte';
export { default as PieChart } from './PieChart.svelte';
export { default as ScatterChart } from './ScatterChart.svelte';

// ============================================================================
// Data Visualization Components
// ============================================================================
export { default as HeatMap } from './HeatMap.svelte';
export { default as TableChart } from './TableChart.svelte';
export { default as MetricCard } from './MetricCard.svelte';
export { default as ChartCard } from './ChartCard.svelte';

// ============================================================================
// Business & Financial Charts
// ============================================================================
export { default as FunnelChart } from './FunnelChart.svelte';
export { default as WaterfallChart } from './WaterfallChart.svelte';
export { default as GaugeChart } from './GaugeChart.svelte';
export { default as BulletChart } from './BulletChart.svelte';
export { default as CandlestickChart } from './CandlestickChart.svelte';

// ============================================================================
// Hierarchical & Relational Charts
// ============================================================================
export { default as TreemapChart } from './TreemapChart.svelte';
export { default as SankeyChart } from './SankeyChart.svelte';
export { default as NetworkGraph } from './NetworkGraph.svelte';

// ============================================================================
// Statistical Charts
// ============================================================================
export { default as BoxPlotChart } from './BoxPlotChart.svelte';
export { default as HistogramChart } from './HistogramChart.svelte';
export { default as RadarChart } from './RadarChart.svelte';

// ============================================================================
// Specialty Charts
// ============================================================================
export { default as GanttChart } from './GanttChart.svelte';
export { default as ComboChart } from './ComboChart.svelte';
export { default as SparklineChart } from './SparklineChart.svelte';

// ============================================================================
// Types
// ============================================================================
export type {
	// Core types
	ChartDataPoint,
	ChartSeries,
	ChartConfig,
	ChartJsData,
	ChartJsDataset,
	ChartExportOptions,
	ChartType,
	// Metric
	MetricData,
	// Table
	TableColumn,
	TableRow,
	// Heat Map
	HeatMapDataPoint,
	// Scatter
	ScatterDataPoint,
	ScatterDataset,
	ScatterChartData,
	QuadrantLabels,
	// Area
	AreaChartMode,
	// Bar
	BarChartMode,
	BarChartOrientation,
	// Line
	LineStyle,
	// Funnel
	FunnelData,
	FunnelOrientation,
	// Waterfall
	WaterfallData,
	// Gauge
	GaugeThreshold,
	// Bullet
	BulletData,
	BulletOrientation,
	// Treemap
	TreemapData,
	// Gantt
	GanttTask,
	// Combo
	ComboDataset,
	ComboChartData,
	// Box Plot
	BoxPlotData,
	// Histogram
	HistogramConfig,
	// Sankey
	SankeyNode,
	SankeyLink,
	SankeyData,
	// Radar
	RadarDataset,
	RadarChartData,
	// Network
	NetworkNode,
	NetworkEdge,
	NetworkGraphData,
	// Candlestick
	CandlestickData,
	// Sparkline
	SparklineType
} from './types';
