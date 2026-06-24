import { useEffect, useState } from 'react';
import { useSkillsStore } from '../../stores/skillsStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { useToast } from '../Toast';
import { Button } from '../ui/Button/Button';

/**
 * 技能管理 — surfaces the skills subsystem that was already fully built
 * server-side (list_skills / skill_catalog / install_skill_from_catalog /
 * uninstall_skill, plus the `skills` SQLite table) but had only a "即将推出"
 * placeholder here.
 *
 * The kernel loads SKILL.md files into ToolRegistry at agent build time, so a
 * skill on disk is already usable by the agent. This page lets the user SEE
 * what's installed, browse the discoverable catalog (global ~/.agents/skills +
 * project-local .agents/skills), and install/uninstall registry entries.
 */
export function SkillsSection() {
  const {
    installed,
    catalog,
    loading,
    loadInstalled,
    loadCatalog,
    installFromCatalog,
    uninstall,
  } = useSkillsStore();
  const activeProject = useNavigationStore((s) => s.activeProject);
  const { success, error } = useToast();

  useEffect(() => {
    loadInstalled();
    loadCatalog(activeProject?.path);
  }, [loadInstalled, loadCatalog, activeProject]);

  // Per-skill in-flight install flags so the button shows "安装中…" + disables
  // while the (idempotent) backend call resolves — without this the await gives
  // no feedback and felt like a hang. Cleared in finally so a failure still
  // re-enables the button.
  const [installing, setInstalling] = useState<Set<string>>(new Set());
  const onInstall = async (name: string, source: string) => {
    setInstalling((prev) => new Set(prev).add(name));
    try {
      const skill = await installFromCatalog(name, source);
      // Surface WHERE it landed so "不知道装到哪" is answered inline — the
      // backend returns path (catalog skills resolve from ~/.agents/skills or
      // project .agents/skills).
      success(skill.path ? `已安装技能 ${name}（路径：${skill.path}）` : `已安装技能 ${name}`);
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setInstalling((prev) => {
        const next = new Set(prev);
        next.delete(name);
        return next;
      });
    }
  };

  const onUninstall = async (id: string, name: string) => {
    try {
      await uninstall(id);
      success(`已卸载技能 ${name}`);
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  const installedNames = new Set(installed.map((s) => s.name));

  return (
    <div className="settings-section skills-section">
      <h3 className="settings-section-title">技能管理</h3>
      <p className="settings-section-desc">
        内核构建 agent 时从 ~/.dev-workbench/skills 与项目 .agents/skills 加载 SKILL.md，作为可调用工具。这里查看已安装技能、浏览可发现目录、安装/卸载。
      </p>

      {/* Installed skills */}
      <div className="skills-subhead">已安装（{installed.length}）</div>
      {loading && installed.length === 0 && <p className="settings-section-desc">加载中...</p>}
      {!loading && installed.length === 0 && (
        <div className="memory-empty">
          <p>暂无已安装技能——从下方目录安装，或把 SKILL.md 放入 skills 目录</p>
        </div>
      )}
      <div className="memory-list">
        {installed.map((s) => (
          <div key={s.id} className="memory-card skills-card">
            <div className="memory-card-header">
              <span className="memory-card-title">{s.name}</span>
              {s.category && <span className="memory-card-category">{s.category}</span>}
              {s.rating != null && (
                <span className="memory-card-confidence">★ {s.rating.toFixed(1)}</span>
              )}
            </div>
            {s.description && <p className="memory-card-content">{s.description}</p>}
            <div className="memory-card-meta">
              <span>{s.org}</span>
              {s.installedAt && <span>· {(s.installedAt || '').slice(0, 10)}</span>}
              <Button
                variant="dangerGhost"
                size="sm"
                onClick={() => onUninstall(s.id, s.name)}
                aria-label={`卸载技能 ${s.name}`}
              >
                卸载
              </Button>
            </div>
          </div>
        ))}
      </div>

      {/* Discoverable catalog */}
      <div className="skills-subhead">可发现目录（{catalog.length}）</div>
      {catalog.length === 0 && (
        <div className="memory-empty">
          <p>目录为空——将 SKILL.md 放入 ~/.agents/skills 或项目 .agents/skills 即可发现</p>
        </div>
      )}
      <div className="memory-list">
        {catalog.map((c) => {
          const isInstalled = installedNames.has(c.name);
          return (
            <div key={`${c.scope}:${c.name}`} className="memory-card skills-card">
              <div className="memory-card-header">
                <span className="memory-card-title">{c.name}</span>
                <span className={`memory-card-category cat-${c.scope}`}>
                  {c.scope === 'global' ? '全局' : '项目'}
                </span>
                {isInstalled && <span className="memory-card-confidence">已安装</span>}
              </div>
              {c.description && <p className="memory-card-content">{c.description}</p>}
              <div className="memory-card-meta">
                <span className="skills-source" title={c.source}>{c.source}</span>
                <Button
                  variant="primary"
                  disabled={isInstalled || installing.has(c.name)}
                  onClick={() => onInstall(c.name, c.source)}
                  aria-label={`安装技能 ${c.name}`}
                >
                  {isInstalled ? '已安装' : installing.has(c.name) ? '安装中…' : '安装'}
                </Button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
