<script lang="ts">
  import Activity from 'lucide-svelte/icons/activity';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import BarChart3 from 'lucide-svelte/icons/bar-chart-3';
  import BookOpen from 'lucide-svelte/icons/book-open';
  import Calendar from 'lucide-svelte/icons/calendar';
  import Check from 'lucide-svelte/icons/check';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import Clock from 'lucide-svelte/icons/clock';
  import Code from 'lucide-svelte/icons/code';
  import Eye from 'lucide-svelte/icons/eye';
  import FileText from 'lucide-svelte/icons/file-text';
  import GitMerge from 'lucide-svelte/icons/git-merge';
  import GitPullRequest from 'lucide-svelte/icons/git-pull-request';
  import Globe from 'lucide-svelte/icons/globe';
  import Hash from 'lucide-svelte/icons/hash';
  import Heart from 'lucide-svelte/icons/heart';
  import LayoutDashboard from 'lucide-svelte/icons/layout-dashboard';
  import Mail from 'lucide-svelte/icons/mail';
  import MessageSquare from 'lucide-svelte/icons/message-square';
  import Pencil from 'lucide-svelte/icons/pencil';
  import Plus from 'lucide-svelte/icons/plus';
  import Search from 'lucide-svelte/icons/search';
  import Send from 'lucide-svelte/icons/send';
  import Share2 from 'lucide-svelte/icons/share-2';
  import Star from 'lucide-svelte/icons/star';
  import Target from 'lucide-svelte/icons/target';
  import ThumbsUp from 'lucide-svelte/icons/thumbs-up';
  import TrendingUp from 'lucide-svelte/icons/trending-up';
  import Users from 'lucide-svelte/icons/users';
  import Zap from 'lucide-svelte/icons/zap';
  import type { Component } from 'svelte';

  let { name = '', size = 16, class: className = '' }: {
    name?: string;
    size?: number;
    class?: string;
  } = $props();

  const iconMap: Record<string, Component> = {
    activity: Activity,
    'alert-triangle': AlertTriangle,
    'arrow-right': ArrowRight,
    'bar-chart-3': BarChart3,
    'book-open': BookOpen,
    calendar: Calendar,
    check: Check,
    'chevron-right': ChevronRight,
    clock: Clock,
    code: Code,
    eye: Eye,
    'file-text': FileText,
    'git-merge': GitMerge,
    'git-pull-request': GitPullRequest,
    globe: Globe,
    hash: Hash,
    heart: Heart,
    'layout-dashboard': LayoutDashboard,
    mail: Mail,
    'message-square': MessageSquare,
    pencil: Pencil,
    plus: Plus,
    search: Search,
    send: Send,
    'share-2': Share2,
    star: Star,
    target: Target,
    'thumbs-up': ThumbsUp,
    'trending-up': TrendingUp,
    users: Users,
    zap: Zap,
  };

  const icon = $derived(iconMap[name]);
</script>

{#if icon}
  {@const Icon = icon}
  <Icon {size} class={className} />
{/if}
