import type { A2UIViewsConfig } from '../types.js';

const marketer: A2UIViewsConfig = {
  _nav: [
    { viewId: 'campaigns', label: 'Campaigns', icon: 'zap' },
    { viewId: 'content', label: 'Content', icon: 'file-text' },
    { viewId: 'metrics', label: 'Metrics', icon: 'bar-chart-3' },
    { viewId: 'calendar', label: 'Calendar', icon: 'calendar' },
  ],

  campaigns: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'campaigns-card'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-active', 'stat-leads', 'stat-conversion', 'stat-spend'] },
      { id: 'stat-active', component: 'stat', props: { label: { path: '/metrics/active/label' }, value: { path: '/metrics/active/value' }, change: { path: '/metrics/active/change' } } },
      { id: 'stat-leads', component: 'stat', props: { label: { path: '/metrics/leads/label' }, value: { path: '/metrics/leads/value' }, change: { path: '/metrics/leads/change' } } },
      { id: 'stat-conversion', component: 'stat', props: { label: { path: '/metrics/conversion/label' }, value: { path: '/metrics/conversion/value' }, change: { path: '/metrics/conversion/change' } } },
      { id: 'stat-spend', component: 'stat', props: { label: { path: '/metrics/spend/label' }, value: { path: '/metrics/spend/value' }, change: { path: '/metrics/spend/change' } } },
      { id: 'campaigns-card', component: 'card', children: ['campaigns-inner'] },
      { id: 'campaigns-inner', component: 'column', props: { gap: '2' }, children: ['campaigns-label', 'campaigns-list'] },
      { id: 'campaigns-label', component: 'text', props: { text: 'Active Campaigns', variant: 'label' } },
      { id: 'campaigns-list', component: 'column', props: { gap: '2' }, children: { componentId: 'campaign-card', path: '/campaigns' } },
      { id: 'campaign-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['campaign-inner'] },
      { id: 'campaign-inner', component: 'column', props: { gap: '1' }, children: ['campaign-header', 'campaign-stats', 'campaign-progress'] },
      { id: 'campaign-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['campaign-name', 'campaign-status'] },
      { id: 'campaign-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'campaign-status', component: 'badge', props: { text: { path: 'status' }, variant: { path: 'statusVariant' } } },
      { id: 'campaign-stats', component: 'row', props: { gap: '4' }, children: ['campaign-channel', 'campaign-sent', 'campaign-opens', 'campaign-clicks'] },
      { id: 'campaign-channel', component: 'text', props: { text: { path: 'channel' }, variant: 'caption' } },
      { id: 'campaign-sent', component: 'text', props: { text: { path: 'sentLabel' }, variant: 'mono' } },
      { id: 'campaign-opens', component: 'text', props: { text: { path: 'opensLabel' }, variant: 'mono' } },
      { id: 'campaign-clicks', component: 'text', props: { text: { path: 'clicksLabel' }, variant: 'mono' } },
      { id: 'campaign-progress', component: 'text', props: { text: { path: 'progressLabel' }, variant: 'caption' } },
    ],
    data: {
      metrics: {
        active: { label: 'Active Campaigns', value: '7', change: '2 launching this week' },
        leads: { label: 'MQLs This Month', value: '342', change: '+28% vs last month' },
        conversion: { label: 'Conversion Rate', value: '3.8%', change: '+0.4pp improvement' },
        spend: { label: 'Monthly Spend', value: '$12.4K', change: '$1.2K under budget' },
      },
      campaigns: [
        { name: 'V2 Launch Sequence', status: 'Active', statusVariant: 'success', channel: 'Email + LinkedIn', sentLabel: '2,400 sent', opensLabel: '42% opens', clicksLabel: '8.3% CTR', progressLabel: 'Day 3 of 7 \u2014 nurture sequence' },
        { name: 'Enterprise Outbound', status: 'Active', statusVariant: 'success', channel: 'Email', sentLabel: '180 sent', opensLabel: '38% opens', clicksLabel: '12% CTR', progressLabel: 'Week 2 \u2014 follow-up wave' },
        { name: 'Developer Community', status: 'Draft', statusVariant: 'warning', channel: 'Twitter/X + GitHub', sentLabel: '0 sent', opensLabel: '\u2014', clicksLabel: '\u2014', progressLabel: 'Launches in 3 days' },
        { name: 'Webinar: Agent Workflows', status: 'Scheduled', statusVariant: 'info', channel: 'Email + Landing Page', sentLabel: '0 sent', opensLabel: '\u2014', clicksLabel: '\u2014', progressLabel: 'May 8, 2:00 PM ET' },
      ],
    },
  },

  content: {
    components: [
      { id: 'root', component: 'column', children: ['content-label', 'content-list'] },
      { id: 'content-label', component: 'text', props: { text: 'Marketing Content', variant: 'label' } },
      { id: 'content-list', component: 'column', props: { gap: '2' }, children: { componentId: 'content-card', path: '/contentItems' } },
      { id: 'content-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['content-inner'] },
      { id: 'content-inner', component: 'column', props: { gap: '1' }, children: ['content-header', 'content-desc', 'content-meta'] },
      { id: 'content-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['content-title', 'content-type'] },
      { id: 'content-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'content-type', component: 'badge', props: { text: { path: 'type' }, variant: { path: 'typeVariant' } } },
      { id: 'content-desc', component: 'text', props: { text: { path: 'description' }, variant: 'body' } },
      { id: 'content-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
    ],
    data: {
      contentItems: [
        { title: 'V2 Launch Email Series', type: 'Email', typeVariant: 'info', description: '7-part drip campaign introducing V2 features to existing users', meta: '7 emails \u00b7 Last updated 2h ago' },
        { title: 'Agent Platform Comparison', type: 'Landing Page', typeVariant: 'success', description: 'Interactive comparison page: Nebo vs CrewAI vs AutoGen vs LangGraph', meta: 'Published Apr 26' },
        { title: 'Case Study: Acme Corp', type: 'PDF', typeVariant: 'warning', description: 'How Acme Corp reduced ops overhead by 40% with Nebo agents', meta: 'Draft \u00b7 Awaiting client approval' },
        { title: 'Product Tour Video', type: 'Video', typeVariant: 'accent', description: '3-minute walkthrough of V2 workspace and agent configuration', meta: 'In production \u00b7 ETA May 2' },
        { title: 'ROI Calculator', type: 'Tool', typeVariant: '', description: 'Interactive calculator showing agent time savings vs manual workflow', meta: 'Design phase' },
      ],
    },
  },

  metrics: {
    components: [
      { id: 'root', component: 'column', children: ['funnel-stats', 'channel-card'] },
      { id: 'funnel-stats', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-visitors', 'stat-signups', 'stat-trials', 'stat-paid'] },
      { id: 'stat-visitors', component: 'stat', props: { label: { path: '/funnel/visitors/label' }, value: { path: '/funnel/visitors/value' }, change: { path: '/funnel/visitors/change' } } },
      { id: 'stat-signups', component: 'stat', props: { label: { path: '/funnel/signups/label' }, value: { path: '/funnel/signups/value' }, change: { path: '/funnel/signups/change' } } },
      { id: 'stat-trials', component: 'stat', props: { label: { path: '/funnel/trials/label' }, value: { path: '/funnel/trials/value' }, change: { path: '/funnel/trials/change' } } },
      { id: 'stat-paid', component: 'stat', props: { label: { path: '/funnel/paid/label' }, value: { path: '/funnel/paid/value' }, change: { path: '/funnel/paid/change' } } },
      { id: 'channel-card', component: 'card', children: ['channel-inner'] },
      { id: 'channel-inner', component: 'column', props: { gap: '2' }, children: ['channel-label', 'channel-list'] },
      { id: 'channel-label', component: 'text', props: { text: 'Channel Performance', variant: 'label' } },
      { id: 'channel-list', component: 'column', props: { gap: '0' }, children: { componentId: 'channel-row', path: '/channels' } },
      { id: 'channel-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['channel-name', 'channel-traffic', 'channel-conversions', 'channel-cac'] },
      { id: 'channel-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'channel-traffic', component: 'text', props: { text: { path: 'traffic' }, variant: 'mono', class: 'w-[80px] text-right' } },
      { id: 'channel-conversions', component: 'text', props: { text: { path: 'conversions' }, variant: 'mono', class: 'w-[80px] text-right' } },
      { id: 'channel-cac', component: 'text', props: { text: { path: 'cac' }, variant: 'mono', class: 'w-[80px] text-right' } },
    ],
    data: {
      funnel: {
        visitors: { label: 'Website Visitors', value: '18.2K', change: '+22% MoM' },
        signups: { label: 'Sign-ups', value: '847', change: '4.6% conversion' },
        trials: { label: 'Active Trials', value: '234', change: '27.6% trial rate' },
        paid: { label: 'Paid Conversions', value: '42', change: '17.9% trial-to-paid' },
      },
      channels: [
        { name: 'Organic Search', traffic: '8.4K', conversions: '312', cac: '$0' },
        { name: 'LinkedIn Ads', traffic: '3.2K', conversions: '186', cac: '$34' },
        { name: 'Twitter/X', traffic: '2.8K', conversions: '142', cac: '$18' },
        { name: 'Email', traffic: '2.1K', conversions: '124', cac: '$8' },
        { name: 'GitHub/Dev', traffic: '1.7K', conversions: '83', cac: '$0' },
      ],
    },
  },

  calendar: {
    components: [
      { id: 'root', component: 'column', children: ['cal-label', 'events-list'] },
      { id: 'cal-label', component: 'text', props: { text: 'Marketing Calendar', variant: 'label' } },
      { id: 'events-list', component: 'column', props: { gap: '2' }, children: { componentId: 'event-card', path: '/events' } },
      { id: 'event-card', component: 'card', children: ['event-inner'] },
      { id: 'event-inner', component: 'row', props: { align: 'center', gap: '3' }, children: ['event-date', 'event-info', 'event-type'] },
      { id: 'event-date', component: 'text', props: { text: { path: 'date' }, variant: 'mono', class: 'w-[80px] shrink-0' } },
      { id: 'event-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['event-title', 'event-desc'] },
      { id: 'event-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'event-desc', component: 'text', props: { text: { path: 'description' }, variant: 'caption' } },
      { id: 'event-type', component: 'badge', props: { text: { path: 'type' }, variant: { path: 'typeVariant' } } },
    ],
    data: {
      events: [
        { date: 'May 1', title: 'V2 Launch Email Blast', description: 'Send to full user base (12K contacts)', type: 'Email', typeVariant: 'info' },
        { date: 'May 3', title: 'Product Hunt Launch', description: 'Coordinated launch with press and social', type: 'Launch', typeVariant: 'success' },
        { date: 'May 5', title: 'Developer Community AMA', description: 'Reddit + Discord Q&A session', type: 'Event', typeVariant: 'accent' },
        { date: 'May 8', title: 'Webinar: Agent Workflows', description: 'Live demo with Q&A, targeting enterprise leads', type: 'Webinar', typeVariant: 'warning' },
        { date: 'May 12', title: 'Case Study Publish', description: 'Acme Corp success story across all channels', type: 'Content', typeVariant: '' },
        { date: 'May 15', title: 'Monthly Newsletter', description: 'V2 recap, new features, community highlights', type: 'Email', typeVariant: 'info' },
      ],
    },
  },
};

export default marketer;
