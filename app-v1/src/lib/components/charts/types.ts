/**
 * Common chart type definitions for Ballast analytics
 * All charts are pure SVG implementations - no external dependencies
 */

// ============================================================================
// Core Types
// ============================================================================

export interface ChartDataPoint {
	x: number | string | Date;
	y: number;
	label?: string;
}

export interface ChartSeries {
	label: string;
	data: ChartDataPoint[];
	color?: string;
	borderColor?: string;
	backgroundColor?: string;
}

export interface ChartConfig {
	title?: string;
	subtitle?: string;
	description?: string;
	xLabel?: string;
	yLabel?: string;
	showLegend?: boolean;
	showGrid?: boolean;
	showTooltips?: boolean;
	responsive?: boolean;
	maintainAspectRatio?: boolean;
	aspectRatio?: number;
}

// ============================================================================
// Chart.js Compatible Data Format (used by most charts)
// ============================================================================

export interface ChartJsDataset {
	label?: string;
	data: number[];
	borderColor?: string;
	backgroundColor?: string | string[];
	fill?: boolean;
	confidenceUpper?: number[];
	confidenceLower?: number[];
}

export interface ChartJsData {
	labels?: string[];
	datasets?: ChartJsDataset[];
}

// ============================================================================
// Metric Card
// ============================================================================

export interface MetricData {
	title: string;
	value: string | number;
	trend?: number;
	trendPeriod?: string;
	format?: 'number' | 'currency' | 'percentage' | 'duration';
	icon?: any;
	color?: string;
	description?: string;
}

// ============================================================================
// Table Chart
// ============================================================================

export interface TableColumn {
	key: string;
	label: string;
	sortable?: boolean;
	align?: 'left' | 'center' | 'right';
	format?: (value: any) => string;
}

export interface TableRow {
	[key: string]: any;
}

// ============================================================================
// Heat Map
// ============================================================================

export interface HeatMapDataPoint {
	x: string;
	y: string;
	value: number;
	label?: string;
}

// ============================================================================
// Scatter Chart
// ============================================================================

export interface ScatterDataPoint {
	x: number;
	y: number;
	r?: number;
	label?: string;
}

export interface ScatterDataset {
	label?: string;
	data: ScatterDataPoint[];
	borderColor?: string;
	backgroundColor?: string;
}

export interface ScatterChartData {
	datasets: ScatterDataset[];
}

export interface QuadrantLabels {
	topLeft?: string;
	topRight?: string;
	bottomLeft?: string;
	bottomRight?: string;
}

// ============================================================================
// Area Chart
// ============================================================================

export type AreaChartMode = 'stacked' | 'stacked100' | 'overlap';

// ============================================================================
// Bar Chart
// ============================================================================

export type BarChartMode = 'grouped' | 'stacked' | 'stacked100' | 'diverging';
export type BarChartOrientation = 'vertical' | 'horizontal';

// ============================================================================
// Line Chart
// ============================================================================

export type LineStyle = 'solid' | 'step' | 'stepBefore' | 'stepAfter';

// ============================================================================
// Funnel Chart
// ============================================================================

export interface FunnelData {
	label: string;
	value: number;
	color?: string;
}

export type FunnelOrientation = 'vertical' | 'horizontal';

// ============================================================================
// Waterfall Chart
// ============================================================================

export interface WaterfallData {
	label: string;
	value: number;
	type?: 'increase' | 'decrease' | 'total';
}

// ============================================================================
// Gauge Chart
// ============================================================================

export interface GaugeThreshold {
	value: number;
	color: string;
	label?: string;
}

// ============================================================================
// Bullet Chart
// ============================================================================

export interface BulletData {
	label: string;
	value: number;
	target?: number;
	ranges?: number[];
	rangeColors?: string[];
}

export type BulletOrientation = 'horizontal' | 'vertical';

// ============================================================================
// Treemap Chart
// ============================================================================

export interface TreemapData {
	label: string;
	value: number;
	color?: string;
	children?: TreemapData[];
}

// ============================================================================
// Gantt Chart
// ============================================================================

export interface GanttTask {
	id: string;
	label: string;
	start: Date | string;
	end: Date | string;
	progress?: number;
	color?: string;
	dependencies?: string[];
}

// ============================================================================
// Combo Chart
// ============================================================================

export interface ComboDataset {
	label?: string;
	data: number[];
	type: 'bar' | 'line';
	borderColor?: string;
	backgroundColor?: string;
	yAxisID?: 'y' | 'y1';
	fill?: boolean;
}

export interface ComboChartData {
	labels?: string[];
	datasets?: ComboDataset[];
}

// ============================================================================
// Box Plot Chart
// ============================================================================

export interface BoxPlotData {
	label: string;
	min: number;
	q1: number;
	median: number;
	q3: number;
	max: number;
	outliers?: number[];
	color?: string;
}

// ============================================================================
// Histogram Chart
// ============================================================================

export interface HistogramConfig {
	binCount?: number;
	showNormalCurve?: boolean;
	color?: string;
}

// ============================================================================
// Sankey Chart
// ============================================================================

export interface SankeyNode {
	id: string;
	label: string;
	color?: string;
}

export interface SankeyLink {
	source: string;
	target: string;
	value: number;
}

export interface SankeyData {
	nodes: SankeyNode[];
	links: SankeyLink[];
}

// ============================================================================
// Radar Chart
// ============================================================================

export interface RadarDataset {
	label?: string;
	data: number[];
	borderColor?: string;
	backgroundColor?: string;
}

export interface RadarChartData {
	labels: string[];
	datasets: RadarDataset[];
}

// ============================================================================
// Network Graph
// ============================================================================

export interface NetworkNode {
	id: string;
	label: string;
	size?: number;
	color?: string;
	group?: string;
}

export interface NetworkEdge {
	source: string;
	target: string;
	weight?: number;
	label?: string;
}

export interface NetworkGraphData {
	nodes: NetworkNode[];
	edges: NetworkEdge[];
}

// ============================================================================
// Candlestick Chart
// ============================================================================

export interface CandlestickData {
	date: Date | string;
	open: number;
	high: number;
	low: number;
	close: number;
	volume?: number;
}

// ============================================================================
// Sparkline Chart
// ============================================================================

export type SparklineType = 'line' | 'bar' | 'area';

// ============================================================================
// Export Options
// ============================================================================

export interface ChartExportOptions {
	format: 'png' | 'csv' | 'json';
	filename?: string;
}

// ============================================================================
// Chart Type Enum (for dynamic chart rendering)
// ============================================================================

export type ChartType =
	| 'bar'
	| 'line'
	| 'area'
	| 'pie'
	| 'donut'
	| 'scatter'
	| 'heatmap'
	| 'table'
	| 'metric'
	| 'funnel'
	| 'waterfall'
	| 'gauge'
	| 'bullet'
	| 'treemap'
	| 'gantt'
	| 'combo'
	| 'boxplot'
	| 'histogram'
	| 'sankey'
	| 'radar'
	| 'network'
	| 'candlestick'
	| 'sparkline';
