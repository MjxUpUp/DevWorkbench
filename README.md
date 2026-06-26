# 一目了然 / Dev Workbench

<div align="center">

**面向 vibecoding 开发者的本地项目工作台**

管理并行开发项目 · 内置 Agent 内核与工作流编排 · 一键启动工具链

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-green.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)]()
[![Tauri](https://img.shields.io/badge/Tauri-2-orange.svg)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61dafb.svg)](https://react.dev)
[![Version](https://img.shields.io/badge/Version-1.0.5-blue.svg)](https://github.com/MjxUpUp/DevWorkbench/releases)

</div>

<!-- 截图占位：建议放一张主界面截图或动图
![Dev Workbench 主界面](docs/screenshot.png)
-->

## 功能特性

### 项目工作台
- **项目管理** — 添加/编辑/删除/收藏/搜索，支持标签分类
- **目录扫描** — 自动发现指定目录下的 Git 仓库
- **工具自动检测** — 启动时扫描系统已安装的开发工具
- **一键启动工具链** — Claude Code、Cursor、VS Code、Terminal、Finder

### Agent 内核
- **自研内核** — 基于 PTY 的真实进程执行，流式对话渲染
- **多模型支持** — Claude / GLM / Gemini / Qwen 等可配置接入
- **工作流编排** — Agent 自规划 DAG，多任务串并行执行
- **多 Agent 协同** — 跨 Agent 任务协同工作台

### 开发者工具
- **内置终端** — 不离开应用即可执行命令
- **Git 面板** — 查看变更与状态
- **知识库** — 项目级知识沉淀
- **MCP 集成** — 接入 MCP 服务器扩展能力
- **质量报告** — 任务质量可视化

### 体验
- **多主题** — 含 Obsidian Terminal 暗色设计语言
- **命令面板** — 快速检索与跳转
- **跨平台** — Windows / macOS / Linux
- **自动更新** — 内置更新器，新版本自动提示

## 下载安装

从 [GitHub Releases](https://github.com/MjxUpUp/DevWorkbench/releases) 下载对应平台的安装包：

| 平台 | 安装包 |
|------|--------|
| Windows | `.exe`（NSIS 安装包） |
| macOS | `.dmg` / `.app` |
| Linux | `.AppImage` / `.deb` |

安装后应用会自动检查并提示新版本。

## 快速使用

1. 启动应用，添加你的项目目录
2. 在设置中配置模型连接（API Key / 本地模型）
3. 选择项目，打开 Agent 对话或工作流编排
4. 一键启动 Claude Code / Cursor / VS Code 等工具链

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
├── components/
│   ├── chat/                 # Agent 对话界面
│   ├── orchestrate/          # 工作流 DAG 编排
│   ├── dashboard/            # 仪表盘
│   ├── git/                  # Git 面板
│   ├── trace/                # 任务追踪
│   ├── settings/             # 设置面板
│   ├── TerminalView.tsx      # 内置终端
│   ├── KnowledgeCard.tsx     # 知识库卡片
│   ├── McpServerList.tsx     # MCP 服务器列表
│   ├── ModelSelector.tsx     # 模型选择
│   ├── QualityReportPanel.tsx# 质量报告
│   ├── CommandPalette.tsx    # 命令面板
│   └── ...                   # 项目卡片、工具按钮、图标等
├── hooks/                    # 自定义 Hooks（项目 CRUD、工具检测等）
├── stores/                   # 状态管理
├── types/                    # TypeScript 类型定义
└── styles/                   # 全局样式

src-tauri/src/                # 后端（Rust）
├── lib.rs / main.rs          # Tauri 入口与命令挂载
├── kernel_impl/              # 自研 Agent 内核
├── agents/                   # Agent 执行（PTY、流式）
├── commands/                 # Tauri 命令（工具检测、终端、扫描等）
├── acp/                      # Agent 通信协议
├── skills/                   # 技能系统
├── mcp/                      # MCP 集成
├── knowledge/                # 知识库
├── quality/                  # 质量治理
├── trace/                    # 任务追踪
├── config/                   # 配置
├── cost/                     # 成本统计
└── models.rs                 # 数据模型
```

## 更新日志

| 版本 | 日期 | 主要变更 |
|------|------|----------|
| v1.0.5 | 2026-06-26 | 跨会话记忆隔离、技能入口统一到设置页、流式 thinking 块落库 |
| v1.0.4 | 2026-06-25 | Windows Git Bash 检测修复、续聊实时渲染、Workflow 图反序列化修复 |
| v1.0.0 | 2026-06-16 | Gemini / Qwen Code 结构化流式接入、UTF-8 多字节截断 panic 修复 |
| v0.8.0 | 2026-06-12 | Clean Professional 设计重构 + 功能对齐 |
| v0.7.0 | 2026-06-11 | 跨 Agent 协同工作台 |
| v0.6.0 | 2026-06-09 | 基于 PTY 的 Agent 执行架构、Agent 面板与项目卡片 |
| v0.5.0 | 2026-06-07 | 多主题系统、紧凑卡片布局、终端检测、macOS 工具检测修复 |
| v0.4.0 | 2026-06-07 | 自动技术栈检测、Pi/Codex 支持 |
| v0.3.0 | 2026-06-07 | 跨平台打包修复（macOS updater / Windows bundle / 产物命名）|

## License

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-green.svg)](LICENSE)

本项目基于 [Apache License 2.0](LICENSE) 开源。

Copyright 2026 MjxUpUp
