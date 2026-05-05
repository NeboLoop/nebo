import type { A2UIViewsConfig } from '../types.js';

const inboxManager: A2UIViewsConfig = {
  _nav: [
    { viewId: 'triage', label: 'Triage', icon: 'inbox' },
    { viewId: 'drafts', label: 'Drafts', icon: 'pencil' },
    { viewId: 'vips', label: 'VIPs', icon: 'star' },
    { viewId: 'stats', label: 'Stats', icon: 'bar-chart-3' },
  ],

  triage: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'inbox-card'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-unread', 'stat-flagged', 'stat-drafts', 'stat-processed'] },
      { id: 'stat-unread', component: 'stat', props: { label: { path: '/metrics/unread/label' }, value: { path: '/metrics/unread/value' }, change: { path: '/metrics/unread/change' } } },
      { id: 'stat-flagged', component: 'stat', props: { label: { path: '/metrics/flagged/label' }, value: { path: '/metrics/flagged/value' }, change: { path: '/metrics/flagged/change' } } },
      { id: 'stat-drafts', component: 'stat', props: { label: { path: '/metrics/drafts/label' }, value: { path: '/metrics/drafts/value' }, change: { path: '/metrics/drafts/change' } } },
      { id: 'stat-processed', component: 'stat', props: { label: { path: '/metrics/processed/label' }, value: { path: '/metrics/processed/value' }, change: { path: '/metrics/processed/change' } } },
      { id: 'inbox-card', component: 'card', children: ['inbox-inner'] },
      { id: 'inbox-inner', component: 'column', props: { gap: '2' }, children: ['inbox-header', 'inbox-list'] },
      { id: 'inbox-header', component: 'row', props: { justify: 'between', align: 'center' }, children: ['inbox-label', 'triage-btn'] },
      { id: 'inbox-label', component: 'text', props: { text: 'Needs Triage', variant: 'label' } },
      { id: 'triage-btn', component: 'button', props: { label: 'Triage All', variant: 'primary' }, action: { name: 'triage-all' } },
      { id: 'inbox-list', component: 'column', props: { gap: '0' }, children: { componentId: 'email-row', path: '/untriaged' } },
      { id: 'email-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-3 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-200/50 transition-colors' }, children: ['email-priority', 'email-info', 'email-category', 'email-time'] },
      { id: 'email-priority', component: 'dot', props: { variant: { path: 'priority' } } },
      { id: 'email-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['email-from', 'email-subject'] },
      { id: 'email-from', component: 'text', props: { text: { path: 'from' }, variant: 'body-medium' } },
      { id: 'email-subject', component: 'text', props: { text: { path: 'subject' }, variant: 'caption' } },
      { id: 'email-category', component: 'badge', props: { text: { path: 'suggestedLabel' }, variant: { path: 'labelVariant' } } },
      { id: 'email-time', component: 'text', props: { text: { path: 'time' }, variant: 'meta' } },
    ],
    data: {
      metrics: {
        unread: { label: 'Unread', value: '23', change: '12 arrived overnight' },
        flagged: { label: 'Flagged', value: '5', change: 'VIP senders' },
        drafts: { label: 'Drafts Ready', value: '3', change: 'Awaiting your review' },
        processed: { label: 'Processed Today', value: '47', change: 'Auto-labeled & sorted' },
      },
      untriaged: [
        { from: 'Alex Kim (CloudNine AI)', subject: 'Follow-up on pricing discussion', priority: 'warning', suggestedLabel: 'Sales', labelVariant: 'accent', time: '8:42 AM' },
        { from: 'GitHub', subject: '[nebo] Issue #892: Memory leak in WebSocket handler', priority: 'warning', suggestedLabel: 'Engineering', labelVariant: 'info', time: '8:15 AM' },
        { from: 'Newsletter \u2014 The Pragmatic Engineer', subject: 'Issue #142: AI Agents in Production', priority: 'success', suggestedLabel: 'Reading', labelVariant: '', time: '7:00 AM' },
        { from: 'Emma Wilson (BrightPath)', subject: 'Partnership meeting \u2014 available Thursday?', priority: 'warning', suggestedLabel: 'Schedule', labelVariant: 'warning', time: '6:30 AM' },
        { from: 'AWS', subject: 'Your April invoice is ready', priority: 'success', suggestedLabel: 'Billing', labelVariant: '', time: 'Yesterday' },
        { from: 'Ryan Taylor', subject: 'Introduction: VP Engineering at ScaleWise', priority: 'warning', suggestedLabel: 'Networking', labelVariant: 'info', time: 'Yesterday' },
      ],
    },
  },

  drafts: {
    components: [
      { id: 'root', component: 'column', props: { gap: '3' }, children: { componentId: 'draft-card', path: '/drafts' } },
      { id: 'draft-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['draft-inner'] },
      { id: 'draft-inner', component: 'column', props: { gap: '2' }, children: ['draft-header', 'draft-preview', 'draft-actions'] },
      { id: 'draft-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['draft-to', 'draft-status'] },
      { id: 'draft-to', component: 'text', props: { text: { path: 'to' }, variant: 'body-medium' } },
      { id: 'draft-status', component: 'badge', props: { text: { path: 'status' }, variant: { path: 'statusVariant' } } },
      { id: 'draft-preview', component: 'text', props: { text: { path: 'preview' }, variant: 'body' } },
      { id: 'draft-actions', component: 'row', props: { gap: '2' }, children: ['send-btn', 'edit-btn'] },
      { id: 'send-btn', component: 'button', props: { label: 'Send', variant: 'primary' }, action: { name: 'send-draft' } },
      { id: 'edit-btn', component: 'button', props: { label: 'Edit', variant: 'default' }, action: { name: 'edit-draft' } },
    ],
    data: {
      drafts: [
        { to: 'To: Alex Kim (CloudNine AI)', preview: 'Hi Alex, thanks for the follow-up. I\'ve reviewed the pricing tiers and have a few questions about the enterprise plan...', status: 'Ready', statusVariant: 'success' },
        { to: 'To: Emma Wilson (BrightPath)', preview: 'Hi Emma, Thursday at 2 PM works great. I\'ll send a calendar invite with the Zoom link. Looking forward to discussing...', status: 'Ready', statusVariant: 'success' },
        { to: 'To: Ryan Taylor (ScaleWise)', preview: 'Ryan, thanks for the introduction. I\'d love to connect \u2014 are you free for a 15-minute call next week?', status: 'Needs Review', statusVariant: 'warning' },
      ],
    },
  },

  vips: {
    components: [
      { id: 'root', component: 'column', children: ['vip-label', 'vip-list'] },
      { id: 'vip-label', component: 'text', props: { text: 'VIP Senders', variant: 'label' } },
      { id: 'vip-list', component: 'column', props: { gap: '0' }, children: { componentId: 'vip-row', path: '/vips' } },
      { id: 'vip-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-3 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-200/50 transition-colors' }, children: ['vip-info', 'vip-unread', 'vip-last'] },
      { id: 'vip-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['vip-name', 'vip-email'] },
      { id: 'vip-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'vip-email', component: 'text', props: { text: { path: 'email' }, variant: 'caption' } },
      { id: 'vip-unread', component: 'badge', props: { text: { path: 'unreadLabel' }, variant: { path: 'unreadVariant' } } },
      { id: 'vip-last', component: 'text', props: { text: { path: 'lastContact' }, variant: 'meta' } },
    ],
    data: {
      vips: [
        { name: 'Sarah Chen', email: 'sarah@acme.co', unreadLabel: '2 unread', unreadVariant: 'error', lastContact: 'Today' },
        { name: 'James Park', email: 'james@board.co', unreadLabel: '1 unread', unreadVariant: 'warning', lastContact: 'Today' },
        { name: 'Mark Thompson', email: 'mark@advisor.co', unreadLabel: '', unreadVariant: '', lastContact: 'Yesterday' },
        { name: 'Legal Team', email: 'legal@company.co', unreadLabel: '', unreadVariant: '', lastContact: '2d ago' },
        { name: 'Alex Kim', email: 'alex@cloudnine.ai', unreadLabel: '1 unread', unreadVariant: 'warning', lastContact: 'Today' },
      ],
    },
  },

  stats: {
    components: [
      { id: 'root', component: 'column', children: ['weekly-stats', 'categories-card'] },
      { id: 'weekly-stats', component: 'row', props: { class: 'grid grid-cols-3' }, children: ['stat-received', 'stat-replied', 'stat-avg-time'] },
      { id: 'stat-received', component: 'stat', props: { label: { path: '/weeklyMetrics/received/label' }, value: { path: '/weeklyMetrics/received/value' }, change: { path: '/weeklyMetrics/received/change' } } },
      { id: 'stat-replied', component: 'stat', props: { label: { path: '/weeklyMetrics/replied/label' }, value: { path: '/weeklyMetrics/replied/value' }, change: { path: '/weeklyMetrics/replied/change' } } },
      { id: 'stat-avg-time', component: 'stat', props: { label: { path: '/weeklyMetrics/avgTime/label' }, value: { path: '/weeklyMetrics/avgTime/value' }, change: { path: '/weeklyMetrics/avgTime/change' } } },
      { id: 'categories-card', component: 'card', children: ['categories-inner'] },
      { id: 'categories-inner', component: 'column', props: { gap: '2' }, children: ['categories-label', 'categories-list'] },
      { id: 'categories-label', component: 'text', props: { text: 'Top Categories This Week', variant: 'label' } },
      { id: 'categories-list', component: 'column', props: { gap: '0' }, children: { componentId: 'cat-row', path: '/categories' } },
      { id: 'cat-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['cat-name', 'cat-count', 'cat-pct'] },
      { id: 'cat-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'cat-count', component: 'text', props: { text: { path: 'count' }, variant: 'mono', class: 'w-[60px] text-right' } },
      { id: 'cat-pct', component: 'text', props: { text: { path: 'pct' }, variant: 'caption', class: 'w-[50px] text-right' } },
    ],
    data: {
      weeklyMetrics: {
        received: { label: 'Emails Received', value: '312', change: '-8% vs last week' },
        replied: { label: 'Replies Sent', value: '89', change: 'Auto-drafted: 67' },
        avgTime: { label: 'Avg Response Time', value: '18 min', change: 'Down from 42 min' },
      },
      categories: [
        { name: 'Client Communication', count: '84', pct: '27%' },
        { name: 'Internal', count: '72', pct: '23%' },
        { name: 'Notifications', count: '61', pct: '20%' },
        { name: 'Newsletters', count: '45', pct: '14%' },
        { name: 'Sales Inquiries', count: '32', pct: '10%' },
        { name: 'Other', count: '18', pct: '6%' },
      ],
    },
  },
};

export default inboxManager;
