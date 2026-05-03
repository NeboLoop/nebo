import type { A2UIViewsConfig } from '../types.js';

const social: A2UIViewsConfig = {
  _nav: [
    { viewId: 'calendar', label: 'Calendar', icon: 'calendar' },
    { viewId: 'drafts', label: 'Drafts', icon: 'pencil' },
    { viewId: 'published', label: 'Published', icon: 'check' },
    { viewId: 'analytics', label: 'Analytics', icon: 'bar-chart-3' },
  ],

  calendar: {
    components: [
      { id: 'root', component: 'column', children: ['schedule-card', 'upcoming-card'] },
      { id: 'schedule-card', component: 'card', children: ['schedule-inner'] },
      { id: 'schedule-inner', component: 'column', props: { gap: '3' }, children: ['schedule-header', 'week-grid'] },
      { id: 'schedule-header', component: 'row', props: { justify: 'between', align: 'center' }, children: ['schedule-title', 'nav-buttons'] },
      { id: 'schedule-title', component: 'text', props: { text: 'This Week', variant: 'body-medium' } },
      { id: 'nav-buttons', component: 'row', props: { gap: '1' }, children: ['btn-today'] },
      { id: 'btn-today', component: 'button', props: { label: 'Today', variant: 'default' }, action: { name: 'goto-today' } },
      { id: 'week-grid', component: 'row', props: { gap: '2' }, children: { componentId: 'day-col', path: '/weekDays' } },
      { id: 'day-col', component: 'column', props: { gap: '1', class: 'flex-1 rounded-lg bg-base-100 p-2.5 min-h-[100px]' }, children: ['day-label', 'day-posts'] },
      { id: 'day-label', component: 'text', props: { text: { path: 'label' }, variant: 'mono' } },
      { id: 'day-posts', component: 'column', props: { gap: '1' }, children: { componentId: 'post-chip', path: 'posts' } },
      { id: 'post-chip', component: 'text', props: { text: { path: 'platform' }, variant: 'body-medium', class: 'rounded py-0.5 px-1.5 bg-base-200/50 truncate' } },
      { id: 'upcoming-card', component: 'card', children: ['upcoming-inner'] },
      { id: 'upcoming-inner', component: 'column', props: { gap: '2' }, children: ['upcoming-label', 'upcoming-list'] },
      { id: 'upcoming-label', component: 'text', props: { text: 'Upcoming Posts', variant: 'label' } },
      { id: 'upcoming-list', component: 'column', props: { gap: '0' }, children: { componentId: 'upcoming-row', path: '/upcomingPosts' } },
      { id: 'upcoming-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['upcoming-platform', 'upcoming-info', 'upcoming-status'] },
      { id: 'upcoming-platform', component: 'text', props: { text: { path: 'platform' }, variant: 'body-medium', class: 'py-0.5 px-2 rounded bg-base-100 border border-base-300 shrink-0 w-[72px] text-center' } },
      { id: 'upcoming-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['upcoming-title', 'upcoming-time'] },
      { id: 'upcoming-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'upcoming-time', component: 'text', props: { text: { path: 'time' }, variant: 'caption' } },
      { id: 'upcoming-status', component: 'badge', props: { text: { path: 'status' }, variant: { path: 'statusVariant' } } },
    ],
    data: {
      weekDays: [
        { label: 'Mon 28', posts: [{ platform: 'LinkedIn' }, { platform: 'Twitter/X' }] },
        { label: 'Tue 29', posts: [{ platform: 'Blog' }] },
        { label: 'Wed 30', posts: [{ platform: 'LinkedIn' }, { platform: 'Twitter/X' }, { platform: 'Blog' }] },
        { label: 'Thu 1', posts: [] },
        { label: 'Fri 2', posts: [{ platform: 'LinkedIn' }, { platform: 'Twitter/X' }] },
        { label: 'Sat 3', posts: [{ platform: 'Blog' }] },
        { label: 'Sun 4', posts: [] },
      ],
      upcomingPosts: [
        { platform: 'LinkedIn', title: 'How AI agents are reshaping SaaS workflows', time: 'Today, 2:00 PM', status: 'ready', statusVariant: 'success' },
        { platform: 'Twitter/X', title: 'Thread: 5 lessons from building an agent platform', time: 'Today, 4:30 PM', status: 'ready', statusVariant: 'success' },
        { platform: 'LinkedIn', title: 'Nebo V2 launch announcement', time: 'Wed, 10:00 AM', status: 'draft', statusVariant: '' },
        { platform: 'Blog', title: 'The future of agent-to-agent communication', time: 'Thu, 9:00 AM', status: 'draft', statusVariant: '' },
      ],
    },
  },

  drafts: {
    components: [
      { id: 'root', component: 'column', props: { gap: '3' }, children: { componentId: 'draft-card', path: '/drafts' } },
      { id: 'draft-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['draft-inner'] },
      { id: 'draft-inner', component: 'column', props: { gap: '2' }, children: ['draft-header', 'draft-meta', 'draft-actions'] },
      { id: 'draft-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['draft-title', 'draft-platform'] },
      { id: 'draft-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'draft-platform', component: 'badge', props: { text: { path: 'platform' } } },
      { id: 'draft-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'draft-actions', component: 'row', props: { gap: '2' }, children: ['edit-btn', 'schedule-btn'] },
      { id: 'edit-btn', component: 'button', props: { label: 'Edit', variant: 'accent' }, action: { name: 'edit-draft' } },
      { id: 'schedule-btn', component: 'button', props: { label: 'Schedule', variant: 'default' }, action: { name: 'schedule-draft' } },
    ],
    data: {
      drafts: [
        { title: 'Nebo V2 launch announcement', platform: 'LinkedIn', meta: '280 words \u00b7 Updated 2h ago' },
        { title: 'The future of agent-to-agent communication', platform: 'Blog', meta: '1240 words \u00b7 Updated 5h ago' },
        { title: 'Agent marketplace best practices', platform: 'Twitter/X', meta: '180 words \u00b7 Updated 1d ago' },
      ],
    },
  },

  published: {
    components: [
      { id: 'root', component: 'card', children: ['pub-list'] },
      { id: 'pub-list', component: 'column', props: { gap: '0' }, children: { componentId: 'pub-row', path: '/published' } },
      { id: 'pub-row', component: 'row', props: { align: 'center', gap: '3', class: 'px-2 py-3 border-b border-base-300 last:border-0' }, children: ['pub-info', 'pub-stats'] },
      { id: 'pub-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['pub-title', 'pub-meta'] },
      { id: 'pub-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'pub-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'pub-stats', component: 'row', props: { gap: '4', class: 'shrink-0' }, children: ['pub-likes', 'pub-comments', 'pub-shares'] },
      { id: 'pub-likes', component: 'text', props: { text: { path: 'likesLabel' }, variant: 'mono' } },
      { id: 'pub-comments', component: 'text', props: { text: { path: 'commentsLabel' }, variant: 'mono' } },
      { id: 'pub-shares', component: 'text', props: { text: { path: 'sharesLabel' }, variant: 'mono' } },
    ],
    data: {
      published: [
        { title: 'Why we built Nebo', meta: 'LinkedIn \u00b7 Apr 25', likesLabel: '142 likes', commentsLabel: '23 comments', sharesLabel: '18 shares' },
        { title: 'Agent orchestration 101', meta: 'Blog \u00b7 Apr 22', likesLabel: '89 likes', commentsLabel: '12 comments', sharesLabel: '34 shares' },
        { title: 'Thread: Building in public', meta: 'Twitter/X \u00b7 Apr 20', likesLabel: '234 likes', commentsLabel: '45 comments', sharesLabel: '67 shares' },
        { title: 'Hiring: Senior Rust engineer', meta: 'LinkedIn \u00b7 Apr 18', likesLabel: '67 likes', commentsLabel: '8 comments', sharesLabel: '15 shares' },
      ],
    },
  },

  analytics: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'top-posts-card'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-3' }, children: ['stat-reach', 'stat-engagement', 'stat-followers'] },
      { id: 'stat-reach', component: 'stat', props: { label: { path: '/metrics/reach/label' }, value: { path: '/metrics/reach/value' }, change: { path: '/metrics/reach/change' } } },
      { id: 'stat-engagement', component: 'stat', props: { label: { path: '/metrics/engagement/label' }, value: { path: '/metrics/engagement/value' }, change: { path: '/metrics/engagement/change' } } },
      { id: 'stat-followers', component: 'stat', props: { label: { path: '/metrics/followers/label' }, value: { path: '/metrics/followers/value' }, change: { path: '/metrics/followers/change' } } },
      { id: 'top-posts-card', component: 'card', children: ['top-inner'] },
      { id: 'top-inner', component: 'column', props: { gap: '2' }, children: ['top-label', 'top-list'] },
      { id: 'top-label', component: 'text', props: { text: 'Top Performing Posts', variant: 'label' } },
      { id: 'top-list', component: 'column', props: { gap: '0' }, children: { componentId: 'top-row', path: '/topPosts' } },
      { id: 'top-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['top-title', 'top-engagement', 'top-reach'] },
      { id: 'top-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'top-engagement', component: 'text', props: { text: { path: 'engagement' }, variant: 'mono', class: 'w-[60px] text-right' } },
      { id: 'top-reach', component: 'text', props: { text: { path: 'reach' }, variant: 'mono', class: 'w-[50px] text-right' } },
    ],
    data: {
      metrics: {
        reach: { label: 'Total Reach', value: '24.8K', change: '+18% vs last week' },
        engagement: { label: 'Engagement Rate', value: '4.2%', change: '+0.8pp vs avg' },
        followers: { label: 'Followers Gained', value: '+187', change: 'Across all platforms' },
      },
      topPosts: [
        { title: 'Thread: Building in public', engagement: '8.4%', reach: '5.2K' },
        { title: 'Why we built Nebo', engagement: '6.1%', reach: '3.8K' },
        { title: 'Agent orchestration 101', engagement: '5.3%', reach: '2.9K' },
      ],
    },
  },
};

export default social;
