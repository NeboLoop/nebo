import type { A2UIViewsConfig } from '../types.js';

const researcher: A2UIViewsConfig = {
  _nav: [
    { viewId: 'overview', label: 'Overview', icon: 'layout-dashboard' },
    { viewId: 'topics', label: 'Topics', icon: 'target' },
    { viewId: 'reports', label: 'Reports', icon: 'file-text' },
    { viewId: 'alerts', label: 'Alerts', icon: 'alert-triangle' },
  ],

  overview: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'content-row'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-tam', 'stat-competitors', 'stat-reports', 'stat-alerts'] },
      { id: 'stat-tam', component: 'stat', props: { label: { path: '/metrics/tam/label' }, value: { path: '/metrics/tam/value' }, change: { path: '/metrics/tam/change' } } },
      { id: 'stat-competitors', component: 'stat', props: { label: { path: '/metrics/competitors/label' }, value: { path: '/metrics/competitors/value' }, change: { path: '/metrics/competitors/change' } } },
      { id: 'stat-reports', component: 'stat', props: { label: { path: '/metrics/reports/label' }, value: { path: '/metrics/reports/value' }, change: { path: '/metrics/reports/change' } } },
      { id: 'stat-alerts', component: 'stat', props: { label: { path: '/metrics/alerts/label' }, value: { path: '/metrics/alerts/value' }, change: { path: '/metrics/alerts/change' } } },
      { id: 'content-row', component: 'row', props: { class: 'grid grid-cols-2', align: 'start' }, children: ['reports-card', 'alerts-card'] },
      { id: 'reports-card', component: 'card', children: ['reports-inner'] },
      { id: 'reports-inner', component: 'column', props: { gap: '2' }, children: ['reports-label', 'reports-list'] },
      { id: 'reports-label', component: 'text', props: { text: 'Recent Reports', variant: 'label' } },
      { id: 'reports-list', component: 'column', props: { gap: '0' }, children: { componentId: 'report-row', path: '/recentReports' } },
      { id: 'report-row', component: 'row', props: { align: 'center', class: 'py-2 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-100 transition-colors rounded px-2 -mx-2' }, children: ['report-info', 'report-type'] },
      { id: 'report-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['report-title', 'report-date'] },
      { id: 'report-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'report-date', component: 'text', props: { text: { path: 'date' }, variant: 'caption' } },
      { id: 'report-type', component: 'badge', props: { text: { path: 'type' } } },
      { id: 'alerts-card', component: 'card', children: ['alerts-inner'] },
      { id: 'alerts-inner', component: 'column', props: { gap: '2' }, children: ['alerts-label', 'alerts-list'] },
      { id: 'alerts-label', component: 'text', props: { text: 'Active Alerts', variant: 'label' } },
      { id: 'alerts-list', component: 'column', props: { gap: '0' }, children: { componentId: 'alert-row', path: '/activeAlerts' } },
      { id: 'alert-row', component: 'row', props: { align: 'center', gap: '2', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['alert-dot', 'alert-info'] },
      { id: 'alert-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'alert-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['alert-title', 'alert-time'] },
      { id: 'alert-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
      { id: 'alert-time', component: 'text', props: { text: { path: 'time' }, variant: 'caption' } },
    ],
    data: {
      metrics: {
        tam: { label: 'Market Size (TAM)', value: '$8.2B', change: 'AI agent platforms 2026' },
        competitors: { label: 'Competitors Tracked', value: '12', change: '3 new this month' },
        reports: { label: 'Reports Generated', value: '47', change: '8 this week' },
        alerts: { label: 'Alerts Active', value: '6', change: '2 triggered today' },
      },
      recentReports: [
        { title: 'Q3 Competitive Landscape', date: 'Apr 27', type: 'analysis' },
        { title: 'Pricing Benchmark Update', date: 'Apr 25', type: 'benchmark' },
        { title: 'AI Agent Market Trends', date: 'Apr 22', type: 'trend' },
        { title: 'Customer Churn Analysis', date: 'Apr 20', type: 'analysis' },
      ],
      activeAlerts: [
        { title: 'Competitor X launched new feature', severity: 'error', time: '2h ago' },
        { title: 'Market share shift detected', severity: 'warning', time: '1d ago' },
        { title: 'New entrant in segment', severity: 'success', time: '2d ago' },
      ],
    },
  },

  topics: {
    components: [
      { id: 'root', component: 'column', children: ['competitors-card', 'trends-card'] },
      { id: 'competitors-card', component: 'card', children: ['comp-inner'] },
      { id: 'comp-inner', component: 'column', props: { gap: '2' }, children: ['comp-label', 'comp-list'] },
      { id: 'comp-label', component: 'text', props: { text: 'Competitor Tracker', variant: 'label' } },
      { id: 'comp-list', component: 'column', props: { gap: '0' }, children: { componentId: 'comp-row', path: '/competitors' } },
      { id: 'comp-row', component: 'row', props: { align: 'center', class: 'py-2.5 border-b border-base-300 last:border-0 hover:bg-base-100 transition-colors cursor-pointer' }, children: ['comp-name', 'comp-cat', 'comp-funding', 'comp-hc', 'comp-threat'] },
      { id: 'comp-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'comp-cat', component: 'text', props: { text: { path: 'category' }, variant: 'body', class: 'w-[100px]' } },
      { id: 'comp-funding', component: 'text', props: { text: { path: 'funding' }, variant: 'mono', class: 'w-[100px]' } },
      { id: 'comp-hc', component: 'text', props: { text: { path: 'headcount' }, variant: 'mono', class: 'w-[100px]' } },
      { id: 'comp-threat', component: 'badge', props: { text: { path: 'threat' }, variant: { path: 'threatLevel' } } },
      { id: 'trends-card', component: 'card', children: ['trends-inner'] },
      { id: 'trends-inner', component: 'column', props: { gap: '2' }, children: ['trends-label', 'trends-list'] },
      { id: 'trends-label', component: 'text', props: { text: 'Market Trends', variant: 'label' } },
      { id: 'trends-list', component: 'column', props: { gap: '2' }, children: { componentId: 'trend-item', path: '/trends' } },
      { id: 'trend-item', component: 'card', children: ['trend-inner'] },
      { id: 'trend-inner', component: 'column', props: { gap: '1' }, children: ['trend-header', 'trend-summary', 'trend-date'] },
      { id: 'trend-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['trend-title', 'trend-badge'] },
      { id: 'trend-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'trend-badge', component: 'badge', props: { text: { path: 'direction' }, variant: { path: 'badgeVariant' } } },
      { id: 'trend-summary', component: 'text', props: { text: { path: 'summary' }, variant: 'body' } },
      { id: 'trend-date', component: 'text', props: { text: { path: 'date' }, variant: 'caption' } },
    ],
    data: {
      competitors: [
        { name: 'AgentForce', category: 'Enterprise', funding: '$120M', headcount: '~450', threat: 'High', threatLevel: 'error' },
        { name: 'CrewAI', category: 'Framework', funding: '$18M', headcount: '~40', threat: 'Medium', threatLevel: 'warning' },
        { name: 'AutoGen', category: 'Open Source', funding: 'Microsoft', headcount: '~25', threat: 'Medium', threatLevel: 'warning' },
        { name: 'LangGraph', category: 'Framework', funding: '$25M', headcount: '~60', threat: 'Medium', threatLevel: 'warning' },
        { name: 'Fixie.ai', category: 'Platform', funding: '$17M', headcount: '~30', threat: 'Low', threatLevel: 'success' },
        { name: 'Relevance AI', category: 'Platform', funding: '$15M', headcount: '~35', threat: 'Low', threatLevel: 'success' },
      ],
      trends: [
        { title: 'Agent-to-Agent Communication', direction: 'Rising', badgeVariant: 'success', summary: 'Multi-agent collaboration frameworks seeing rapid adoption. Key players: CrewAI, AutoGen, Nebo.', date: 'Updated Apr 27' },
        { title: 'Deterministic Workflows', direction: 'Rising', badgeVariant: 'success', summary: 'Shift from pure LLM reasoning to structured agent workflows with predictable behavior.', date: 'Updated Apr 25' },
        { title: 'Agent Marketplaces', direction: 'Emerging', badgeVariant: 'warning', summary: 'Platforms enabling third-party agent/skill distribution. Nebo, Salesforce leading.', date: 'Updated Apr 22' },
        { title: 'Standalone AI Apps', direction: 'Declining', badgeVariant: 'error', summary: 'Single-purpose AI tools losing ground to agent platforms with multi-agent capabilities.', date: 'Updated Apr 20' },
      ],
    },
  },

  reports: {
    components: [
      { id: 'root', component: 'column', children: ['header-row', 'reports-list'] },
      { id: 'header-row', component: 'row', props: { justify: 'between', align: 'center' }, children: ['search-placeholder', 'new-report-btn'] },
      { id: 'search-placeholder', component: 'text', props: { text: 'Search reports...', variant: 'caption' } },
      { id: 'new-report-btn', component: 'button', props: { label: '+ New Report', variant: 'accent' }, action: { name: 'new-report' } },
      { id: 'reports-list', component: 'column', props: { gap: '2' }, children: { componentId: 'report-card', path: '/reports' } },
      { id: 'report-card', component: 'row', props: { align: 'center', gap: '3', class: 'rounded-xl bg-base-100 shadow-sm border border-base-300 p-4 cursor-pointer hover:border-base-content/50 transition-all' }, children: ['report-icon', 'report-info', 'report-status'] },
      { id: 'report-icon', component: 'icon', props: { name: 'file-text', size: 20, class: 'text-base-content/50 shrink-0' } },
      { id: 'report-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['report-title', 'report-meta'] },
      { id: 'report-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'report-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'report-status', component: 'badge', props: { text: { path: 'statusLabel' }, variant: { path: 'statusVariant' } } },
    ],
    data: {
      reports: [
        { title: 'Q3 Competitive Landscape Analysis', meta: 'Apr 27, 2026 \u00b7 12 pages', statusLabel: '', statusVariant: '' },
        { title: 'Pricing Benchmark: AI Agent Platforms', meta: 'Apr 25, 2026 \u00b7 8 pages', statusLabel: '', statusVariant: '' },
        { title: 'AI Agent Market Sizing 2026-2030', meta: 'Apr 22, 2026 \u00b7 24 pages', statusLabel: '', statusVariant: '' },
        { title: 'Customer Churn Deep-Dive', meta: 'Apr 20, 2026 \u00b7 6 pages', statusLabel: '', statusVariant: '' },
        { title: 'Enterprise Sales Cycle Analysis', meta: 'In progress', statusLabel: 'Running', statusVariant: 'warning' },
      ],
    },
  },

  alerts: {
    components: [
      { id: 'root', component: 'column', children: ['label', 'alerts-list'] },
      { id: 'label', component: 'text', props: { text: 'Active Alerts', variant: 'label' } },
      { id: 'alerts-list', component: 'column', props: { gap: '2' }, children: { componentId: 'alert-card', path: '/alerts' } },
      { id: 'alert-card', component: 'card', children: ['alert-inner'] },
      { id: 'alert-inner', component: 'row', props: { align: 'center', gap: '2' }, children: ['alert-dot', 'alert-info', 'alert-time'] },
      { id: 'alert-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'alert-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['alert-title', 'alert-desc'] },
      { id: 'alert-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'alert-desc', component: 'text', props: { text: { path: 'description' }, variant: 'caption' } },
      { id: 'alert-time', component: 'text', props: { text: { path: 'time' }, variant: 'meta' } },
    ],
    data: {
      alerts: [
        { title: 'Competitor X launched new feature', description: 'AgentForce released workflow builder matching our V2 feature set', severity: 'error', time: '2h ago' },
        { title: 'Market share shift detected', description: 'CrewAI gained 2.3% in developer mindshare (StackOverflow survey)', severity: 'warning', time: '1d ago' },
        { title: 'New entrant in segment', description: 'Stealth startup "Agentic" raised $12M seed for enterprise agent platform', severity: 'warning', time: '2d ago' },
        { title: 'Pricing change detected', description: 'Relevance AI dropped Pro tier pricing by 30%', severity: 'warning', time: '3d ago' },
        { title: 'Industry report published', description: 'Gartner released 2026 Magic Quadrant for AI Agent Platforms', severity: 'success', time: '4d ago' },
      ],
    },
  },
};

export default researcher;
