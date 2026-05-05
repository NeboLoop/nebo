import type { A2UIViewsConfig } from '../types.js';

const contentStrategist: A2UIViewsConfig = {
  _nav: [
    { viewId: 'pipeline', label: 'Pipeline', icon: 'layers' },
    { viewId: 'calendar', label: 'Calendar', icon: 'calendar' },
    { viewId: 'seo', label: 'SEO', icon: 'search' },
    { viewId: 'performance', label: 'Performance', icon: 'bar-chart-3' },
  ],

  pipeline: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'content-list'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-drafts', 'stat-review', 'stat-published', 'stat-traffic'] },
      { id: 'stat-drafts', component: 'stat', props: { label: { path: '/metrics/drafts/label' }, value: { path: '/metrics/drafts/value' }, change: { path: '/metrics/drafts/change' } } },
      { id: 'stat-review', component: 'stat', props: { label: { path: '/metrics/review/label' }, value: { path: '/metrics/review/value' }, change: { path: '/metrics/review/change' } } },
      { id: 'stat-published', component: 'stat', props: { label: { path: '/metrics/published/label' }, value: { path: '/metrics/published/value' }, change: { path: '/metrics/published/change' } } },
      { id: 'stat-traffic', component: 'stat', props: { label: { path: '/metrics/traffic/label' }, value: { path: '/metrics/traffic/value' }, change: { path: '/metrics/traffic/change' } } },
      { id: 'content-list', component: 'column', props: { gap: '2' }, children: { componentId: 'content-card', path: '/contentItems' } },
      { id: 'content-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['content-inner'] },
      { id: 'content-inner', component: 'column', props: { gap: '1' }, children: ['content-header', 'content-meta', 'content-keywords'] },
      { id: 'content-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['content-title', 'content-status'] },
      { id: 'content-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'content-status', component: 'badge', props: { text: { path: 'status' }, variant: { path: 'statusVariant' } } },
      { id: 'content-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'content-keywords', component: 'row', props: { gap: '1' }, children: { componentId: 'keyword-badge', path: 'keywords' } },
      { id: 'keyword-badge', component: 'badge', props: { text: { path: 'label' } } },
    ],
    data: {
      metrics: {
        drafts: { label: 'In Draft', value: '6', change: '2 started this week' },
        review: { label: 'In Review', value: '3', change: 'SEO scores ready' },
        published: { label: 'Published (May)', value: '12', change: 'On track for 15 target' },
        traffic: { label: 'Organic Traffic', value: '8.4K', change: '+22% MoM' },
      },
      contentItems: [
        { title: 'Complete Guide to AI Agent Workflows', status: 'Draft', statusVariant: 'warning', meta: 'Blog \u00b7 2,400 words \u00b7 Updated 3h ago', keywords: [{ label: 'ai agents' }, { label: 'workflows' }] },
        { title: 'Nebo vs CrewAI: Head-to-Head Comparison', status: 'In Review', statusVariant: 'info', meta: 'Landing Page \u00b7 SEO Score: 82/100', keywords: [{ label: 'comparison' }, { label: 'crewai' }] },
        { title: 'How to Automate Your Morning Briefing', status: 'Scheduled', statusVariant: 'success', meta: 'Blog \u00b7 Publishes May 6', keywords: [{ label: 'automation' }, { label: 'briefing' }] },
        { title: '10 Ways AI Agents Save Time for Realtors', status: 'Draft', statusVariant: 'warning', meta: 'Blog \u00b7 1,100 words \u00b7 Outline phase', keywords: [{ label: 'real estate' }, { label: 'productivity' }] },
        { title: 'Agent Platform Security Best Practices', status: 'Published', statusVariant: '', meta: 'Blog \u00b7 Published May 1 \u00b7 342 views', keywords: [{ label: 'security' }, { label: 'enterprise' }] },
      ],
    },
  },

  calendar: {
    components: [
      { id: 'root', component: 'column', children: ['cal-card', 'queue-card'] },
      { id: 'cal-card', component: 'card', children: ['cal-inner'] },
      { id: 'cal-inner', component: 'column', props: { gap: '3' }, children: ['cal-header', 'week-grid'] },
      { id: 'cal-header', component: 'text', props: { text: 'Content Calendar \u2014 This Week', variant: 'body-medium' } },
      { id: 'week-grid', component: 'row', props: { gap: '2' }, children: { componentId: 'day-col', path: '/weekDays' } },
      { id: 'day-col', component: 'column', props: { gap: '1', class: 'flex-1 rounded-lg bg-base-100 p-2.5 min-h-[100px]' }, children: ['day-label', 'day-items'] },
      { id: 'day-label', component: 'text', props: { text: { path: 'label' }, variant: 'mono' } },
      { id: 'day-items', component: 'column', props: { gap: '1' }, children: { componentId: 'day-item', path: 'items' } },
      { id: 'day-item', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium', class: 'rounded py-0.5 px-1.5 bg-base-200/50 truncate' } },
      { id: 'queue-card', component: 'card', children: ['queue-inner'] },
      { id: 'queue-inner', component: 'column', props: { gap: '2' }, children: ['queue-label', 'queue-list'] },
      { id: 'queue-label', component: 'text', props: { text: 'Publishing Queue', variant: 'label' } },
      { id: 'queue-list', component: 'column', props: { gap: '0' }, children: { componentId: 'queue-row', path: '/queue' } },
      { id: 'queue-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['queue-date', 'queue-info', 'queue-channel'] },
      { id: 'queue-date', component: 'text', props: { text: { path: 'date' }, variant: 'mono', class: 'w-[70px] shrink-0' } },
      { id: 'queue-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['queue-title', 'queue-status'] },
      { id: 'queue-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'queue-status', component: 'text', props: { text: { path: 'status' }, variant: 'caption' } },
      { id: 'queue-channel', component: 'badge', props: { text: { path: 'channel' } } },
    ],
    data: {
      weekDays: [
        { label: 'Mon', items: [{ title: 'Blog Post' }] },
        { label: 'Tue', items: [{ title: 'LinkedIn' }, { title: 'Twitter' }] },
        { label: 'Wed', items: [] },
        { label: 'Thu', items: [{ title: 'Blog Post' }, { title: 'Email' }] },
        { label: 'Fri', items: [{ title: 'LinkedIn' }] },
      ],
      queue: [
        { date: 'May 5', title: 'Complete Guide to AI Agent Workflows', status: 'Final review needed', channel: 'Blog' },
        { date: 'May 6', title: 'How to Automate Your Morning Briefing', status: 'Ready to publish', channel: 'Blog' },
        { date: 'May 6', title: 'Briefing automation tips (thread)', status: 'Ready', channel: 'Twitter/X' },
        { date: 'May 7', title: 'AI agents for non-technical professionals', status: 'Draft complete', channel: 'LinkedIn' },
        { date: 'May 8', title: 'Weekly newsletter: Agent workflows edition', status: 'Template ready', channel: 'Email' },
      ],
    },
  },

  seo: {
    components: [
      { id: 'root', component: 'column', children: ['seo-stats', 'keywords-card'] },
      { id: 'seo-stats', component: 'row', props: { class: 'grid grid-cols-3' }, children: ['stat-ranking', 'stat-keywords', 'stat-backlinks'] },
      { id: 'stat-ranking', component: 'stat', props: { label: { path: '/seoMetrics/ranking/label' }, value: { path: '/seoMetrics/ranking/value' }, change: { path: '/seoMetrics/ranking/change' } } },
      { id: 'stat-keywords', component: 'stat', props: { label: { path: '/seoMetrics/keywords/label' }, value: { path: '/seoMetrics/keywords/value' }, change: { path: '/seoMetrics/keywords/change' } } },
      { id: 'stat-backlinks', component: 'stat', props: { label: { path: '/seoMetrics/backlinks/label' }, value: { path: '/seoMetrics/backlinks/value' }, change: { path: '/seoMetrics/backlinks/change' } } },
      { id: 'keywords-card', component: 'card', children: ['keywords-inner'] },
      { id: 'keywords-inner', component: 'column', props: { gap: '2' }, children: ['keywords-label', 'keywords-list'] },
      { id: 'keywords-label', component: 'text', props: { text: 'Target Keywords', variant: 'label' } },
      { id: 'keywords-list', component: 'column', props: { gap: '0' }, children: { componentId: 'kw-row', path: '/keywords' } },
      { id: 'kw-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['kw-term', 'kw-position', 'kw-volume', 'kw-trend'] },
      { id: 'kw-term', component: 'text', props: { text: { path: 'term' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'kw-position', component: 'text', props: { text: { path: 'position' }, variant: 'mono', class: 'w-[60px] text-right' } },
      { id: 'kw-volume', component: 'text', props: { text: { path: 'volume' }, variant: 'mono', class: 'w-[80px] text-right' } },
      { id: 'kw-trend', component: 'badge', props: { text: { path: 'trend' }, variant: { path: 'trendVariant' } } },
    ],
    data: {
      seoMetrics: {
        ranking: { label: 'Avg Position', value: '14.2', change: 'Up 3 positions this month' },
        keywords: { label: 'Ranking Keywords', value: '247', change: '+34 new keywords' },
        backlinks: { label: 'Backlinks', value: '1,842', change: '+89 this month' },
      },
      keywords: [
        { term: 'ai agent platform', position: '#8', volume: '2.4K/mo', trend: 'Rising', trendVariant: 'success' },
        { term: 'personal ai assistant desktop', position: '#5', volume: '1.8K/mo', trend: 'Stable', trendVariant: 'info' },
        { term: 'ai workflow automation', position: '#12', volume: '3.2K/mo', trend: 'Rising', trendVariant: 'success' },
        { term: 'nebo ai', position: '#1', volume: '890/mo', trend: 'Stable', trendVariant: 'info' },
        { term: 'ai agents for business', position: '#18', volume: '4.1K/mo', trend: 'New', trendVariant: 'warning' },
        { term: 'crewai alternative', position: '#6', volume: '1.2K/mo', trend: 'Rising', trendVariant: 'success' },
      ],
    },
  },

  performance: {
    components: [
      { id: 'root', component: 'column', children: ['perf-stats', 'top-posts-card'] },
      { id: 'perf-stats', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-views', 'stat-engagement', 'stat-conversions', 'stat-shares'] },
      { id: 'stat-views', component: 'stat', props: { label: { path: '/perfMetrics/views/label' }, value: { path: '/perfMetrics/views/value' }, change: { path: '/perfMetrics/views/change' } } },
      { id: 'stat-engagement', component: 'stat', props: { label: { path: '/perfMetrics/engagement/label' }, value: { path: '/perfMetrics/engagement/value' }, change: { path: '/perfMetrics/engagement/change' } } },
      { id: 'stat-conversions', component: 'stat', props: { label: { path: '/perfMetrics/conversions/label' }, value: { path: '/perfMetrics/conversions/value' }, change: { path: '/perfMetrics/conversions/change' } } },
      { id: 'stat-shares', component: 'stat', props: { label: { path: '/perfMetrics/shares/label' }, value: { path: '/perfMetrics/shares/value' }, change: { path: '/perfMetrics/shares/change' } } },
      { id: 'top-posts-card', component: 'card', children: ['top-inner'] },
      { id: 'top-inner', component: 'column', props: { gap: '2' }, children: ['top-label', 'top-list'] },
      { id: 'top-label', component: 'text', props: { text: 'Top Performing Content', variant: 'label' } },
      { id: 'top-list', component: 'column', props: { gap: '0' }, children: { componentId: 'top-row', path: '/topContent' } },
      { id: 'top-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['top-info', 'top-views', 'top-conv'] },
      { id: 'top-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['top-title', 'top-meta'] },
      { id: 'top-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'top-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'top-views', component: 'text', props: { text: { path: 'views' }, variant: 'mono', class: 'w-[70px] text-right' } },
      { id: 'top-conv', component: 'text', props: { text: { path: 'conversions' }, variant: 'mono', class: 'w-[50px] text-right' } },
    ],
    data: {
      perfMetrics: {
        views: { label: 'Total Views', value: '24.8K', change: '+18% vs last month' },
        engagement: { label: 'Avg Read Time', value: '4:32', change: '+45s vs avg' },
        conversions: { label: 'CTA Clicks', value: '847', change: '3.4% conversion rate' },
        shares: { label: 'Social Shares', value: '312', change: '+67% MoM' },
      },
      topContent: [
        { title: 'Why We Built Nebo', meta: 'Blog \u00b7 Published Apr 15', views: '5.2K', conversions: '234' },
        { title: 'Agent Orchestration 101', meta: 'Blog \u00b7 Published Apr 22', views: '3.8K', conversions: '186' },
        { title: 'AI Agents vs AI Assistants', meta: 'Blog \u00b7 Published Apr 8', views: '3.1K', conversions: '142' },
        { title: 'Nebo Security Whitepaper', meta: 'PDF \u00b7 Published Apr 1', views: '2.4K', conversions: '98' },
        { title: 'Platform Security Best Practices', meta: 'Blog \u00b7 Published May 1', views: '1.9K', conversions: '67' },
      ],
    },
  },
};

export default contentStrategist;
