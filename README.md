# 一目了然 / Dev Workbench

面向 vibecoding 开发者的本地项目工作台 — 管理并行开发项目，一键启动工具链。

## 功能特性

- **项目管理** — 添加/编辑/删除/收藏/搜索，支持标签分类
- **一键启动工具链** — Claude Code、Cursor、VS Code、Terminal、Finder
- **目录扫描** — 自动发现指定目录下的 Git 仓库
- **工具自动检测** — 启动时扫描系统已安装的开发工具
- **暗色主题** — Obsidian Terminal 设计语言，零外部 UI 组件库
- **跨平台** — Windows / macOS / Linux

## 技术栈

| 层 | 技术 |
|---|---|
| 框架 | Tauri 2 |
| 后端 | Rust |
| 前端 | React 19 + TypeScript |
| 构建 | Vite 8 |
| UI | 纯 CSS + 自定义 SVG 图标 |
| 测试 | Vitest + Testing Library（前端）、Rust 原生 test + tempfile（后端）|

## 开发指南

```bash
# 安装依赖
npm install

# 开发模式（前端热更新 + Rust 热重编译）
npm run tauri dev

# 构建安装包
npm run tauri build

# 前端类型检查
npx tsc --noEmit

# 前端测试
npx vitest run

# Rust 测试
cd src-tauri && cargo test
```

## 项目结构

```
src/                          # 前端（React + TypeScript）
├── App.tsx                   # 根组件，视图路由与状态管理
├── main.tsx                  # 入口
├── components/
│   ├── Sidebar.tsx           # 侧边栏导航（全部/最近/收藏）
│   ├── ProjectGrid.tsx       # 项目卡片网格
│   ├── ProjectCard.tsx       # 单个项目卡片
│   ├── AddProject.tsx        # 添加项目对话框
│   ├── Settings.tsx          # 设置面板
│   ├── ToolButton.tsx        # 工具启动按钮
│   └── Icons.tsx             # SVG 图标组件库
├── hooks/
│   ├── useProjects.ts        # 项目 CRUD 与持久化
│   └── useTools.ts           # 工具检测状态
├── types/
│   └── index.ts              # TypeScript 类型定义
├── styles/
│   └── index.css             # 全局样式
└── test/
    └── setup.ts              # 测试环境配置

src-tauri/src/                # 后端（Rust）
├── lib.rs                    # Tauri 插件注册与命令挂载
├── main.rs                   # 入口
├── models.rs                 # 数据模型（Project、Settings、ToolInfo）
└── commands/
    ├── tools.rs              # 工具检测（which/where）
    ├── terminal.rs           # 打开终端
    ├── editor.rs             # 打开编辑器
    ├── finder.rs             # 打开文件管理器
    ├── scanner.rs            # Git 仓库扫描
    └── projects.rs           # 项目与设置的文件读写
```

## License

MIT
