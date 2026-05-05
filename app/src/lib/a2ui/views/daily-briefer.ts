import type { A2UIViewsConfig } from '../types.js';

const dailyBriefer: A2UIViewsConfig = {
  _nav: [
    { viewId: 'today', label: 'Today', icon: 'sun' },
    { viewId: 'news', label: 'News', icon: 'newspaper' },
    { viewId: 'trends', label: 'Trends', icon: 'trending-up' },
    { viewId: 'archive', label: 'Archive', icon: 'archive' },
  ],

  today: {
    components: [
      { id: 'root', component: 'column', children: ['summary-card', 'sections-row'] },
      { id: 'summary-card', component: 'card', children: ['summary-inner'] },
      { id: 'summary-inner', component: 'column', props: { gap: '2' }, children: ['summary-header', 'summary-text'] },
      { id: 'summary-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['summary-title', 'refresh-btn'] },
      { id: 'summary-title', component: 'text', props: { text: { path: '/briefing/title' }, variant: 'body-medium' } },
      { id: 'refresh-btn', component: 'button', props: { label: 'Refresh', variant: 'default' }, action: { name: 'refresh-briefing' } },
      { id: 'summary-text', component: 'text', props: { text: { path: '/briefing/summary' }, variant: 'body' } },
      { id: 'sections-row', component: 'row', props: { class: 'grid grid-cols-2', align: 'start' }, children: ['highlights-card', 'calendar-card'] },
      { id: 'highlights-card', component: 'card', children: ['highlights-inner'] },
      { id: 'highlights-inner', component: 'column', props: { gap: '2' }, children: ['highlights-label', 'highlights-list'] },
      { id: 'highlights-label', component: 'text', props: { text: 'Key Highlights', variant: 'label' } },
      { id: 'highlights-list', component: 'column', props: { gap: '0' }, children: { componentId: 'highlight-row', path: '/highlights' } },
      { id: 'highlight-row', component: 'row', props: { align: 'center', gap: '2', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['highlight-dot', 'highlight-info'] },
      { id: 'highlight-dot', component: 'dot', props: { variant: { path: 'importance' } } },
      { id: 'highlight-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['highlight-title', 'highlight-source'] },
      { id: 'highlight-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
      { id: 'highlight-source', component: 'text', props: { text: { path: 'source' }, variant: 'caption' } },
      { id: 'calendar-card', component: 'card', children: ['calendar-inner'] },
      { id: 'calendar-inner', component: 'column', props: { gap: '2' }, children: ['calendar-label', 'calendar-list'] },
      { id: 'calendar-label', component: 'text', props: { text: "Today's Schedule", variant: 'label' } },
      { id: 'calendar-list', component: 'column', props: { gap: '0' }, children: { componentId: 'cal-row', path: '/schedule' } },
      { id: 'cal-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['cal-time', 'cal-title'] },
      { id: 'cal-time', component: 'text', props: { text: { path: 'time' }, variant: 'mono', class: 'w-[70px] shrink-0' } },
      { id: 'cal-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
    ],
    data: {
      briefing: {
        title: 'Morning Briefing \u2014 Sunday, May 4',
        summary: 'Quiet weekend morning. No urgent emails overnight. Two items flagged for Monday: Acme Corp contract renewal (deadline Tuesday) and board deck final review. Market news: AI agent funding round announcements from two competitors.',
      },
      highlights: [
        { title: 'Acme Corp contract renewal due Tuesday', importance: 'error', source: 'Email \u00b7 Legal team' },
        { title: 'Competitor funding: AgentForce raised $45M Series C', importance: 'warning', source: 'TechCrunch' },
        { title: 'Board deck feedback addressed \u2014 ready for review', importance: 'success', source: 'Internal' },
        { title: 'New product launch from Relevance AI', importance: 'warning', source: 'Product Hunt' },
      ],
      schedule: [
        { time: '10:00 AM', title: 'Weekly review prep (blocked time)' },
        { time: '12:00 PM', title: 'Lunch' },
        { time: '3:00 PM', title: 'Catch-up reading block' },
      ],
    },
  },

  news: {
    components: [
      { id: 'root', component: 'column', children: ['news-label', 'news-list'] },
      { id: 'news-label', component: 'text', props: { text: 'Industry News', variant: 'label' } },
      { id: 'news-list', component: 'column', props: { gap: '2' }, children: { componentId: 'news-card', path: '/articles' } },
      { id: 'news-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['news-inner'] },
      { id: 'news-inner', component: 'column', props: { gap: '1' }, children: ['news-header', 'news-summary', 'news-meta'] },
      { id: 'news-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['news-title', 'news-badge'] },
      { id: 'news-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'news-badge', component: 'badge', props: { text: { path: 'category' }, variant: { path: 'categoryVariant' } } },
      { id: 'news-summary', component: 'text', props: { text: { path: 'summary' }, variant: 'body' } },
      { id: 'news-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
    ],
    data: {
      articles: [
        { title: 'AgentForce raises $45M Series C', category: 'Funding', categoryVariant: 'accent', summary: 'Enterprise agent platform AgentForce closes Series C led by Sequoia. Plans to expand into healthcare and finance verticals.', meta: 'TechCrunch \u00b7 2h ago' },
        { title: 'OpenAI announces agent API improvements', category: 'AI', categoryVariant: 'info', summary: 'New function calling capabilities and structured output support aimed at agent developers.', meta: 'OpenAI Blog \u00b7 5h ago' },
        { title: 'Gartner: AI agents to handle 30% of enterprise tasks by 2028', category: 'Research', categoryVariant: 'warning', summary: 'New report projects rapid adoption of AI agent platforms in enterprise operations.', meta: 'Gartner \u00b7 1d ago' },
        { title: 'Relevance AI launches workflow builder', category: 'Competitor', categoryVariant: 'error', summary: 'No-code workflow builder targets the same SMB market. Free tier includes 100 runs/month.', meta: 'Product Hunt \u00b7 1d ago' },
      ],
    },
  },

  trends: {
    components: [
      { id: 'root', component: 'column', children: ['trends-label', 'trends-list'] },
      { id: 'trends-label', component: 'text', props: { text: 'Weekly Trends', variant: 'label' } },
      { id: 'trends-list', component: 'column', props: { gap: '2' }, children: { componentId: 'trend-card', path: '/trends' } },
      { id: 'trend-card', component: 'card', children: ['trend-inner'] },
      { id: 'trend-inner', component: 'column', props: { gap: '1' }, children: ['trend-header', 'trend-summary'] },
      { id: 'trend-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['trend-title', 'trend-direction'] },
      { id: 'trend-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'trend-direction', component: 'badge', props: { text: { path: 'direction' }, variant: { path: 'directionVariant' } } },
      { id: 'trend-summary', component: 'text', props: { text: { path: 'summary' }, variant: 'body' } },
    ],
    data: {
      trends: [
        { title: 'Agent marketplace platforms', direction: 'Rising', directionVariant: 'success', summary: 'Multiple platforms now offering agent/skill marketplaces. NeboLoop, Salesforce AgentExchange, CrewAI Hub all launched in Q2.' },
        { title: 'MCP protocol adoption', direction: 'Rising', directionVariant: 'success', summary: 'Model Context Protocol gaining traction as standard for agent-tool communication. 40+ integrations in the last month.' },
        { title: 'Standalone AI chatbots', direction: 'Declining', directionVariant: 'error', summary: 'Single-purpose chat UIs losing market share to agent platforms that actually do things.' },
        { title: 'Enterprise agent budgets', direction: 'Emerging', directionVariant: 'warning', summary: 'Fortune 500 companies allocating dedicated budgets for AI agent platforms, separate from general AI spend.' },
      ],
    },
  },

  archive: {
    components: [
      { id: 'root', component: 'column', children: ['archive-label', 'archive-list'] },
      { id: 'archive-label', component: 'text', props: { text: 'Past Briefings', variant: 'label' } },
      { id: 'archive-list', component: 'column', props: { gap: '0' }, children: { componentId: 'archive-row', path: '/pastBriefings' } },
      { id: 'archive-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-3 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-200/50 transition-colors' }, children: ['archive-date', 'archive-info', 'archive-type'] },
      { id: 'archive-date', component: 'text', props: { text: { path: 'date' }, variant: 'mono', class: 'w-[80px] shrink-0' } },
      { id: 'archive-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['archive-title', 'archive-summary'] },
      { id: 'archive-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'archive-summary', component: 'text', props: { text: { path: 'preview' }, variant: 'caption' } },
      { id: 'archive-type', component: 'badge', props: { text: { path: 'type' } } },
    ],
    data: {
      pastBriefings: [
        { date: 'May 3', title: 'Evening Recap', preview: 'Product demo went well. Acme Corp contract sent to legal. 2 new leads.', type: 'Evening' },
        { date: 'May 3', title: 'Morning Briefing', preview: 'Busy day ahead: 6 meetings, 3 urgent emails, board deck due.', type: 'Morning' },
        { date: 'May 2', title: 'Evening Recap', preview: 'Closed deal with CloudNine AI. Engineering sprint wrapped.', type: 'Evening' },
        { date: 'May 2', title: 'Morning Briefing', preview: 'CloudNine final negotiation, sprint review, hiring updates.', type: 'Morning' },
        { date: 'May 1', title: 'Weekly Trends', preview: 'Agent marketplace platforms rising. MCP adoption accelerating.', type: 'Weekly' },
      ],
    },
  },
};

export default dailyBriefer;
