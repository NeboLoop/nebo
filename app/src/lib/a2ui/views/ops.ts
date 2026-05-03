import type { A2UIViewsConfig } from '../types.js';

const ops: A2UIViewsConfig = {
  _nav: [
    { viewId: 'dashboard', label: 'Dashboard', icon: 'layout-dashboard' },
    { viewId: 'contacts', label: 'Contacts', icon: 'users' },
    { viewId: 'pipeline', label: 'Pipeline', icon: 'arrow-right' },
    { viewId: 'activity', label: 'Activity', icon: 'activity' },
  ],

  dashboard: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'pipeline-card', 'activity-card'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-contacts', 'stat-deals', 'stat-pipeline', 'stat-won'] },
      { id: 'stat-contacts', component: 'stat', props: { label: { path: '/metrics/contacts/label' }, value: { path: '/metrics/contacts/value' }, change: { path: '/metrics/contacts/change' } } },
      { id: 'stat-deals', component: 'stat', props: { label: { path: '/metrics/deals/label' }, value: { path: '/metrics/deals/value' }, change: { path: '/metrics/deals/change' } } },
      { id: 'stat-pipeline', component: 'stat', props: { label: { path: '/metrics/pipeline/label' }, value: { path: '/metrics/pipeline/value' }, change: { path: '/metrics/pipeline/change' } } },
      { id: 'stat-won', component: 'stat', props: { label: { path: '/metrics/won/label' }, value: { path: '/metrics/won/value' }, change: { path: '/metrics/won/change' } } },
      { id: 'pipeline-card', component: 'card', children: ['pipeline-inner'] },
      { id: 'pipeline-inner', component: 'column', props: { gap: '3' }, children: ['pipeline-label', 'pipeline-stages'] },
      { id: 'pipeline-label', component: 'text', props: { text: 'Pipeline Stages', variant: 'label' } },
      { id: 'pipeline-stages', component: 'row', props: { gap: '2' }, children: { componentId: 'stage-item', path: '/stages' } },
      { id: 'stage-item', component: 'column', props: { align: 'center', gap: '0', class: 'flex-1 rounded-lg p-3 bg-base-200/50' }, children: ['stage-count', 'stage-name'] },
      { id: 'stage-count', component: 'text', props: { text: { path: 'count' }, variant: 'h3' } },
      { id: 'stage-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'activity-card', component: 'card', children: ['activity-inner'] },
      { id: 'activity-inner', component: 'column', props: { gap: '2' }, children: ['activity-label', 'activity-list'] },
      { id: 'activity-label', component: 'text', props: { text: 'Recent Activity', variant: 'label' } },
      { id: 'activity-list', component: 'column', props: { gap: '0' }, children: { componentId: 'activity-row', path: '/recentActivity' } },
      { id: 'activity-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['activity-info', 'activity-time'] },
      { id: 'activity-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['activity-action', 'activity-contact'] },
      { id: 'activity-action', component: 'text', props: { text: { path: 'action' }, variant: 'body-medium' } },
      { id: 'activity-contact', component: 'text', props: { text: { path: 'contact' }, variant: 'caption' } },
      { id: 'activity-time', component: 'text', props: { text: { path: 'time' }, variant: 'meta' } },
    ],
    data: {
      metrics: {
        contacts: { label: 'Total Contacts', value: '1,247', change: '+38 this month' },
        deals: { label: 'Active Deals', value: '23', change: '4 closing soon' },
        pipeline: { label: 'Pipeline Value', value: '$284K', change: '+12% MoM' },
        won: { label: 'Won This Month', value: '$42K', change: '3 deals closed' },
      },
      stages: [
        { name: 'Lead', count: '45' },
        { name: 'Qualified', count: '18' },
        { name: 'Proposal', count: '8' },
        { name: 'Negotiation', count: '5' },
        { name: 'Closed Won', count: '3' },
      ],
      recentActivity: [
        { action: 'Deal moved to Negotiation', contact: 'Acme Corp', time: '12m ago' },
        { action: 'New contact added', contact: 'Sarah Chen', time: '1h ago' },
        { action: 'Follow-up email sent', contact: 'TechStart Inc', time: '2h ago' },
        { action: 'Meeting scheduled', contact: 'DataFlow Labs', time: '3h ago' },
        { action: 'Deal closed won', contact: 'CloudNine AI', time: '1d ago' },
      ],
    },
  },

  contacts: {
    components: [
      { id: 'root', component: 'column', children: ['contacts-list'] },
      { id: 'contacts-list', component: 'card', children: ['contacts-inner'] },
      { id: 'contacts-inner', component: 'column', props: { gap: '0' }, children: { componentId: 'contact-row', path: '/contacts' } },
      { id: 'contact-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 px-2 border-b border-base-300 last:border-0 hover:bg-base-100 transition-colors cursor-pointer' }, children: ['contact-info', 'contact-company', 'contact-stage', 'contact-value'] },
      { id: 'contact-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['contact-name', 'contact-email'] },
      { id: 'contact-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'contact-email', component: 'text', props: { text: { path: 'email' }, variant: 'caption' } },
      { id: 'contact-company', component: 'text', props: { text: { path: 'company' }, variant: 'body', class: 'w-[140px]' } },
      { id: 'contact-stage', component: 'badge', props: { text: { path: 'stage' }, variant: { path: 'stageVariant' } } },
      { id: 'contact-value', component: 'text', props: { text: { path: 'value' }, variant: 'mono', class: 'w-[80px] text-right' } },
    ],
    data: {
      contacts: [
        { name: 'Sarah Chen', email: 'sarah@acme.co', company: 'Acme Corp', stage: 'Negotiation', stageVariant: 'accent', value: '$48K' },
        { name: 'James Park', email: 'james@techstart.io', company: 'TechStart Inc', stage: 'Proposal', stageVariant: 'warning', value: '$32K' },
        { name: 'Maria Garcia', email: 'maria@dataflow.ai', company: 'DataFlow Labs', stage: 'Qualified', stageVariant: 'info', value: '$65K' },
        { name: 'Alex Kim', email: 'alex@cloudnine.ai', company: 'CloudNine AI', stage: 'Closed Won', stageVariant: 'success', value: '$24K' },
        { name: 'David Liu', email: 'david@nexgen.co', company: 'NexGen', stage: 'Lead', stageVariant: '', value: '$18K' },
        { name: 'Emma Wilson', email: 'emma@brightpath.io', company: 'BrightPath', stage: 'Qualified', stageVariant: 'info', value: '$55K' },
        { name: 'Ryan Taylor', email: 'ryan@scalewise.com', company: 'ScaleWise', stage: 'Proposal', stageVariant: 'warning', value: '$41K' },
      ],
    },
  },

  pipeline: {
    components: [
      { id: 'root', component: 'row', props: { gap: '3', class: 'min-h-[400px]', align: 'start' }, children: { componentId: 'pipeline-col', path: '/columns' } },
      { id: 'pipeline-col', component: 'column', props: { gap: '2', class: 'flex-1 min-w-0' }, children: ['col-header', 'col-deals'] },
      { id: 'col-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['col-stage', 'col-count'] },
      { id: 'col-stage', component: 'text', props: { text: { path: 'stage' }, variant: 'body-medium' } },
      { id: 'col-count', component: 'text', props: { text: { path: 'count' }, variant: 'mono' } },
      { id: 'col-deals', component: 'column', props: { gap: '2' }, children: { componentId: 'deal-card', path: 'deals' } },
      { id: 'deal-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/30 transition-all' }, children: ['deal-inner'] },
      { id: 'deal-inner', component: 'column', props: { gap: '1' }, children: ['deal-name', 'deal-value'] },
      { id: 'deal-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'deal-value', component: 'text', props: { text: { path: 'value' }, variant: 'mono' } },
    ],
    data: {
      columns: [
        { stage: 'Lead', count: '3', deals: [{ name: 'NexGen', value: '$18K' }, { name: 'Orbit AI', value: '$22K' }, { name: 'Flux Labs', value: '$15K' }] },
        { stage: 'Qualified', count: '2', deals: [{ name: 'DataFlow Labs', value: '$65K' }, { name: 'BrightPath', value: '$55K' }] },
        { stage: 'Proposal', count: '2', deals: [{ name: 'TechStart', value: '$32K' }, { name: 'ScaleWise', value: '$41K' }] },
        { stage: 'Negotiation', count: '1', deals: [{ name: 'Acme Corp', value: '$48K' }] },
      ],
    },
  },

  activity: {
    components: [
      { id: 'root', component: 'column', props: { class: 'max-w-[600px]' }, children: ['today-label', 'activity-list'] },
      { id: 'today-label', component: 'text', props: { text: 'Today', variant: 'label' } },
      { id: 'activity-list', component: 'column', props: { gap: '3' }, children: { componentId: 'log-entry', path: '/activityLog' } },
      { id: 'log-entry', component: 'row', props: { align: 'start', gap: '3' }, children: ['log-time', 'log-info'] },
      { id: 'log-time', component: 'text', props: { text: { path: 'time' }, variant: 'mono', class: 'w-[70px] shrink-0 pt-0.5' } },
      { id: 'log-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['log-action', 'log-type'] },
      { id: 'log-action', component: 'text', props: { text: { path: 'action' }, variant: 'body' } },
      { id: 'log-type', component: 'text', props: { text: { path: 'typeLabel' }, variant: 'caption' } },
    ],
    data: {
      activityLog: [
        { time: '9:42 AM', action: 'Ops auto-triaged 3 inbound leads', typeLabel: 'Automated by Ops' },
        { time: '10:15 AM', action: 'Follow-up email sent to Sarah Chen (Acme Corp)', typeLabel: 'Automated by Ops' },
        { time: '11:30 AM', action: 'Meeting notes captured: DataFlow Labs demo', typeLabel: 'Automated by Ops' },
        { time: '1:00 PM', action: 'Deal stage updated: Acme Corp \u2192 Negotiation', typeLabel: 'Manual' },
        { time: '2:45 PM', action: 'New contact enriched: Ryan Taylor (ScaleWise)', typeLabel: 'Automated by Ops' },
      ],
    },
  },
};

export default ops;
