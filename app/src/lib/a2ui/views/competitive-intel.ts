import type { A2UIViewsConfig } from '../types.js';

const competitiveIntel: A2UIViewsConfig = {
  _nav: [
    { viewId: 'dashboard', label: 'Dashboard', icon: 'layout-dashboard' },
    { viewId: 'competitors', label: 'Competitors', icon: 'users' },
    { viewId: 'signals', label: 'Signals', icon: 'radio' },
    { viewId: 'reports', label: 'Reports', icon: 'file-text' },
  ],

  dashboard: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'content-row'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-tracked', 'stat-signals', 'stat-threats', 'stat-opps'] },
      { id: 'stat-tracked', component: 'stat', props: { label: { path: '/metrics/tracked/label' }, value: { path: '/metrics/tracked/value' }, change: { path: '/metrics/tracked/change' } } },
      { id: 'stat-signals', component: 'stat', props: { label: { path: '/metrics/signals/label' }, value: { path: '/metrics/signals/value' }, change: { path: '/metrics/signals/change' } } },
      { id: 'stat-threats', component: 'stat', props: { label: { path: '/metrics/threats/label' }, value: { path: '/metrics/threats/value' }, change: { path: '/metrics/threats/change' } } },
      { id: 'stat-opps', component: 'stat', props: { label: { path: '/metrics/opps/label' }, value: { path: '/metrics/opps/value' }, change: { path: '/metrics/opps/change' } } },
      { id: 'content-row', component: 'row', props: { class: 'grid grid-cols-2', align: 'start' }, children: ['signals-card', 'changes-card'] },
      { id: 'signals-card', component: 'card', children: ['signals-inner'] },
      { id: 'signals-inner', component: 'column', props: { gap: '2' }, children: ['signals-label', 'signals-list'] },
      { id: 'signals-label', component: 'text', props: { text: 'Recent Signals', variant: 'label' } },
      { id: 'signals-list', component: 'column', props: { gap: '0' }, children: { componentId: 'signal-row', path: '/recentSignals' } },
      { id: 'signal-row', component: 'row', props: { align: 'center', gap: '2', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['signal-dot', 'signal-info'] },
      { id: 'signal-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'signal-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['signal-title', 'signal-source'] },
      { id: 'signal-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
      { id: 'signal-source', component: 'text', props: { text: { path: 'source' }, variant: 'caption' } },
      { id: 'changes-card', component: 'card', children: ['changes-inner'] },
      { id: 'changes-inner', component: 'column', props: { gap: '2' }, children: ['changes-label', 'changes-list'] },
      { id: 'changes-label', component: 'text', props: { text: 'Market Movements', variant: 'label' } },
      { id: 'changes-list', component: 'column', props: { gap: '0' }, children: { componentId: 'change-row', path: '/marketChanges' } },
      { id: 'change-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['change-info', 'change-impact'] },
      { id: 'change-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['change-title', 'change-time'] },
      { id: 'change-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
      { id: 'change-time', component: 'text', props: { text: { path: 'time' }, variant: 'caption' } },
      { id: 'change-impact', component: 'badge', props: { text: { path: 'impact' }, variant: { path: 'impactVariant' } } },
    ],
    data: {
      metrics: {
        tracked: { label: 'Competitors Tracked', value: '12', change: '3 new this quarter' },
        signals: { label: 'Signals This Week', value: '18', change: '6 require review' },
        threats: { label: 'Active Threats', value: '3', change: '1 critical' },
        opps: { label: 'Opportunities', value: '5', change: '2 actionable now' },
      },
      recentSignals: [
        { title: 'AgentForce raised $45M Series C', severity: 'error', source: 'TechCrunch \u00b7 2h ago' },
        { title: 'CrewAI hiring 15 engineers', severity: 'warning', source: 'LinkedIn \u00b7 1d ago' },
        { title: 'Relevance AI dropped pricing 30%', severity: 'error', source: 'Product Hunt \u00b7 1d ago' },
        { title: 'LangGraph released v0.4', severity: 'warning', source: 'GitHub \u00b7 2d ago' },
      ],
      marketChanges: [
        { title: 'Enterprise segment: 3 new entrants', time: 'This week', impact: 'High', impactVariant: 'error' },
        { title: 'SMB pricing race accelerating', time: 'This month', impact: 'Medium', impactVariant: 'warning' },
        { title: 'Agent marketplace category emerging', time: 'Q2 2026', impact: 'Opportunity', impactVariant: 'success' },
      ],
    },
  },

  competitors: {
    components: [
      { id: 'root', component: 'column', children: ['comp-label', 'comp-list'] },
      { id: 'comp-label', component: 'text', props: { text: 'Competitor Profiles', variant: 'label' } },
      { id: 'comp-list', component: 'column', props: { gap: '0' }, children: { componentId: 'comp-row', path: '/competitors' } },
      { id: 'comp-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-3 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-200/50 transition-colors' }, children: ['comp-info', 'comp-cat', 'comp-funding', 'comp-hc', 'comp-threat'] },
      { id: 'comp-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['comp-name', 'comp-desc'] },
      { id: 'comp-name', component: 'text', props: { text: { path: 'name' }, variant: 'body-medium' } },
      { id: 'comp-desc', component: 'text', props: { text: { path: 'focus' }, variant: 'caption' } },
      { id: 'comp-cat', component: 'text', props: { text: { path: 'category' }, variant: 'body', class: 'w-[100px]' } },
      { id: 'comp-funding', component: 'text', props: { text: { path: 'funding' }, variant: 'mono', class: 'w-[90px] text-right' } },
      { id: 'comp-hc', component: 'text', props: { text: { path: 'headcount' }, variant: 'mono', class: 'w-[70px] text-right' } },
      { id: 'comp-threat', component: 'badge', props: { text: { path: 'threat' }, variant: { path: 'threatVariant' } } },
    ],
    data: {
      competitors: [
        { name: 'AgentForce', focus: 'Enterprise agent orchestration', category: 'Enterprise', funding: '$120M', headcount: '~450', threat: 'High', threatVariant: 'error' },
        { name: 'CrewAI', focus: 'Multi-agent framework for developers', category: 'Framework', funding: '$18M', headcount: '~40', threat: 'Medium', threatVariant: 'warning' },
        { name: 'AutoGen', focus: 'Microsoft-backed open source agents', category: 'Open Source', funding: 'Microsoft', headcount: '~25', threat: 'Medium', threatVariant: 'warning' },
        { name: 'LangGraph', focus: 'Agent workflow graphs', category: 'Framework', funding: '$25M', headcount: '~60', threat: 'Medium', threatVariant: 'warning' },
        { name: 'Relevance AI', focus: 'No-code AI agent builder', category: 'Platform', funding: '$15M', headcount: '~35', threat: 'Low', threatVariant: 'success' },
        { name: 'Fixie.ai', focus: 'Conversational AI agents', category: 'Platform', funding: '$17M', headcount: '~30', threat: 'Low', threatVariant: 'success' },
      ],
    },
  },

  signals: {
    components: [
      { id: 'root', component: 'column', children: ['signals-header', 'signals-list'] },
      { id: 'signals-header', component: 'row', props: { justify: 'between', align: 'center' }, children: ['signals-label', 'scan-btn'] },
      { id: 'signals-label', component: 'text', props: { text: 'Intelligence Signals', variant: 'label' } },
      { id: 'scan-btn', component: 'button', props: { label: 'Scan Now', variant: 'primary' }, action: { name: 'scan-signals' } },
      { id: 'signals-list', component: 'column', props: { gap: '2' }, children: { componentId: 'signal-card', path: '/signals' } },
      { id: 'signal-card', component: 'card', children: ['signal-inner'] },
      { id: 'signal-inner', component: 'column', props: { gap: '1' }, children: ['signal-header', 'signal-body', 'signal-meta'] },
      { id: 'signal-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['signal-dot', 'signal-title', 'signal-type'] },
      { id: 'signal-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'signal-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'signal-type', component: 'badge', props: { text: { path: 'type' }, variant: { path: 'typeVariant' } } },
      { id: 'signal-body', component: 'text', props: { text: { path: 'description' }, variant: 'body' } },
      { id: 'signal-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
    ],
    data: {
      signals: [
        { title: 'AgentForce Series C: $45M', type: 'Funding', typeVariant: 'accent', severity: 'error', description: 'Led by Sequoia. Plans to expand into healthcare and finance. Valuation reportedly $400M+.', meta: 'TechCrunch \u00b7 2h ago \u00b7 Competitor: AgentForce' },
        { title: 'Relevance AI pricing restructure', type: 'Pricing', typeVariant: 'warning', severity: 'error', description: 'Pro tier dropped 30%. New free tier includes 100 agent runs/month. Targeting SMB aggressively.', meta: 'Product Hunt \u00b7 1d ago \u00b7 Competitor: Relevance AI' },
        { title: 'CrewAI hiring surge', type: 'Hiring', typeVariant: 'info', severity: 'warning', description: '15 open engineering roles, 3 in agent marketplace. Suggests marketplace feature incoming.', meta: 'LinkedIn \u00b7 1d ago \u00b7 Competitor: CrewAI' },
        { title: 'Gartner Magic Quadrant published', type: 'Research', typeVariant: 'info', severity: 'warning', description: 'First-ever MQ for "AI Agent Platforms." AgentForce in Leaders, most others in Niche Players.', meta: 'Gartner \u00b7 3d ago \u00b7 Industry' },
        { title: 'LangGraph v0.4: workflow persistence', type: 'Product', typeVariant: '', severity: 'success', description: 'Added persistent workflow state and human-in-the-loop features. Open source.', meta: 'GitHub \u00b7 2d ago \u00b7 Competitor: LangGraph' },
      ],
    },
  },

  reports: {
    components: [
      { id: 'root', component: 'column', children: ['reports-header', 'reports-list'] },
      { id: 'reports-header', component: 'row', props: { justify: 'between', align: 'center' }, children: ['reports-label', 'new-report-btn'] },
      { id: 'reports-label', component: 'text', props: { text: 'Intelligence Reports', variant: 'label' } },
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
        { title: 'Q2 Competitive Landscape Analysis', meta: 'May 2, 2026 \u00b7 14 pages', statusLabel: '', statusVariant: '' },
        { title: 'AgentForce Deep Dive: Series C Impact', meta: 'May 4, 2026 \u00b7 6 pages', statusLabel: 'New', statusVariant: 'accent' },
        { title: 'Pricing Benchmark: AI Agent Platforms', meta: 'Apr 28, 2026 \u00b7 8 pages', statusLabel: '', statusVariant: '' },
        { title: 'AI Agent Market Sizing 2026-2030', meta: 'Apr 22, 2026 \u00b7 24 pages', statusLabel: '', statusVariant: '' },
        { title: 'Enterprise Feature Comparison Matrix', meta: 'In progress', statusLabel: 'Running', statusVariant: 'warning' },
      ],
    },
  },
};

export default competitiveIntel;
