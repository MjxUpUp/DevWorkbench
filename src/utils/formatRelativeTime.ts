/**
 * Format an ISO timestamp as a Chinese relative time string.
 * Single canonical implementation — all components should use this.
 */
export function formatRelativeTime(isoTime: string): string {
  const now = Date.now();
  const then = new Date(isoTime).getTime();
  const diffSec = Math.floor((now - then) / 1000);

  if (diffSec < 60) return '刚刚';
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  if (diffSec < 2592000) return `${Math.floor(diffSec / 86400)} 天前`;
  if (diffSec < 31536000) return `${Math.floor(diffSec / 2592000)} 个月前`;
  return `${Math.floor(diffSec / 31536000)} 年前`;
}
