# Win Mouse Fix — 开源准备计划

> 版本：v0.1 | 日期：2026-09-07
> 负责人：产品经理（Claude Code）
> 目标：公开可复现、质量可信、有社区增长潜力的开源发布

---

## 一、现状盘点

### 1.1 已具备

| 维度 | 状态 | 说明 |
|---|---|---|
| 核心代码 | ✅ 完整 | P1–P7 + P8–P10 + P13–P16 共 144 测试通过 |
| 许可证 | ✅ MIT | `Cargo.toml` 已声明 |
| 版本号 | ✅ v0.1.0 | `Cargo.toml` 已声明 |
| 构建工具 | ✅ Cargo | Rust 项目标准 |

### 1.2 缺失的关键开源要素

| 缺失项 | 严重度 | 说明 |
|---|---|---|
| **README.md** | 🔴 阻断 | 无项目介绍、安装、使用说明；GitHub 主页空白 |
| **LICENSE 文件** | 🔴 阻断 | 只有 `Cargo.toml` 声明，仓库根目录无 `LICENSE` 文件 |
| **CHANGELOG.md** | 🔴 阻断 | 无版本变更记录，无法追溯发布内容 |
| **CONTRIBUTING.md** | 🔴 阻断 | 无贡献指南，潜在贡献者不知道如何参与 |
| **发布产物** | 🔴 阻断 | 无 `.exe` 可下载，用户无法零构建使用 |
| **徽章体系** | 🟡 高 | 无 CI badge / test badge / license badge |
| **截图 / GIF** | 🟡 高 | 无功能展示，传播物料匮乏 |
| **安全 policy** | 🟡 高 | 无 `SECURITY.md`，漏洞无法上报 |
| **代码签名** | 🟡 中 | Windows 无签名会被系统拦截，需讨论策略 |
| **国际化** | 🟡 中 | 暂无多语言（可英文首发后再扩） |
| **隐私政策** | 🟡 中 | 工具读取电池/按键数据，需说明数据处理 |
| **标签 / topic** | 🟢 低 | GitHub 话题标签配置 |

---

## 二、发布前必做项（v0.1.0 首发阻断清单）

### 2.1 必做 1：创建 `README.md`

结构建议：

```
# Win Mouse Fix
一行 slogan
功能特性列表（带图标）
与 mac-mouse-fix 的关系说明
安装指南（下载 .exe / cargo install）
快速开始（config.toml 说明）
配置示例
截图 / GIF
FAQ
LICENSE
```

关键原则：
- README 是潜在用户的第一印象，必须回答"这是什么 + 我为什么要用"
- 截图（托盘 UI、功能效果）放在 `docs/screenshots/` 目录
- 英文优先（可附中文），覆盖最大受众

### 2.2 必做 2：创建 `LICENSE` 文件

在仓库根目录添加完整的 MIT License 文本（年份 2026，作者名待确认）。

### 2.3 必做 3：创建 `CHANGELOG.md`

格式建议：[Keep a Changelog](https://keepachangelog.com/) 标准格式。

首发 v0.1.0 条目：
- 平滑滚动（双指数平滑，注入管线）
- 按键重映射（RemapEngine + ClickCycleTracker）
- 点击拖拽手势（DragController 三模式）
- 指针加速（PointerAccel）
- 修改键组合（滚轮 + 修饰键联动）
- 设备层（HID++ 电量读取，DPI 档位查询）
- 托盘电池图标（动态百分比叠加）
- DPI 跨屏自动切换
- 手机触控板（WebSocket HTTP 服务 + PWA）
- 日志轮转 + panic hook

### 2.4 必做 4：创建 `CONTRIBUTING.md`

内容：
- 开发环境（Rust 1.75+，Windows SDK）
- `cargo build` / `cargo test` / `cargo clippy` 本地验证步骤
- 代码风格（`cargo fmt` + `clippy`）
- 分支模型（feature branch → PR → review）
- 测试要求（单元测试新增必须，集成测试视情况）
- 问题报告模板（Bug report / Feature request）
- 响应时间预期（开源维护时间有限，诚实说明）

### 2.5 必做 5：构建并发布 `.exe`

发布方式选项：

| 方式 | 优点 | 缺点 |
|---|---|---|
| **GitHub Releases** | 官方、免费、CI 自动 | 需配置 Actions |
| **Chocolatey / winget** | Windows 原生包管理 | 需通过审核 |
| **Scoop** | 社区流行 | 需维护 manifest |

**建议**：首发 GitHub Releases，验证有用户需求后扩到 winget/Scoop。

CI 配置 `release.yml`：
- 触发：`git tag v*` push 时
- 构建：`cargo build --release`
- 产物压缩：`target/release/win-mouse-fix.exe` → `.zip`
- 上传 Release Asset

### 2.6 必做 6：创建 `SECURITY.md`

内容：
- 安全漏洞上报方式（私下报告，不公开 GitHub Issue）
- 数据处理声明（本工具纯本地运行，无网络上传，按键数据不外传）
- 权限说明（管理员权限用途，仅用于全局钩子注入）

---

## 三、发布后建设项（v0.1.x 跟进）

### 3.1 CI/CD 完善

```
✅ release.yml          # GitHub Releases 自动构建
⬜ test.yml             # 每次 PR 运行 cargo test + clippy
⬜ dependabot.yml       # 依赖更新自动化
```

### 3.2 徽章体系

```
[![Build](https://github.com/<user>/win-mouse-fix/actions/workflows/test.yml/badge.svg)]
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]
[![Crates.io](https://img.shields.io/crates/v/win-mouse-fix.svg)]
```

### 3.3 截图 / 功能展示材料

必做截图：
1. 托盘菜单（含电池图标 + 各功能开关）
2. `config.toml` 典型配置（展示各功能段）
3. 手机触控板 PWA 界面（手机端截图）
4. 动态电池图标（低电量红色 + 正常蓝色）
5. 平滑滚动效果对比（可选：GIF 或文字描述）

### 3.4 文档站（可选，v0.2+ 再做）

选项：
- **VitePress**（静态生成，部署 GitHub Pages）
- **Docusaurus**（更重，功能更强）
- 暂时：README + `docs/` 目录足够

---

## 四、社区建设规划

### 4.1 Issues 模板

创建 `.github/ISSUE_TEMPLATE/`:
- `bug_report.md`：复现步骤、环境信息、预期 vs 实际行为
- `feature_request.md`：使用场景、建议方案、替代方案
- `question.md`：一般问题

### 4.2 讨论区策略

- GitHub Discussions：开启，用于 FAQ / 想法收集
- Reddit / Twitter：取决于目标用户社区分布

### 4.3 增长渠道

| 渠道 | 策略 | 优先级 |
|---|---|---|
| GitHub Trending | 首发 + 精准 description | 高 |
| Rust 中文社区 | 博客/帖子介绍 | 中 |
| Hacker News | 可尝试提交 | 中 |
| Reddit r/rust / r/mouse | 社区帖 | 中 |
| V2EX | 中文技术社区 | 低 |

### 4.4 目标用户画像

主要用户：
1. **从 macOS 切换到 Windows 的开发者**：已习惯 Mac Mouse Fix，寻求替代
2. **性能敏感用户**：嫌弃 Windows 鼠标"不够跟手"
3. **程序员 / 设计师**：需要精准滚轮 + 自定义按键
4. **罗技鼠标用户**：配合 G Hub 使用

---

## 五、法律与合规

### 5.1 权限声明

工具需要：
- 管理员权限（`WH_MOUSE_LL` / `WH_KEYBOARD_LL` 全局钩子）
- UAC 提示说明（写入 Release Notes）

### 5.2 数据处理

本工具：
- 所有处理在本地完成，**不联网**
- 按键数据不收集、不上传
- 配置文件仅存储在本地

需要在 README / SECURITY.md 明确声明。

### 5.3 第三方依赖许可证审计

`cargo tree` 检查：
- 所有依赖应为 MIT / Apache-2.0 / BSD 系列兼容许可
- 无 Copyleft（GPL）依赖混入

---

## 六、发布节奏建议

```
v0.1.0 首发（阻断项全部完成）
├── README.md
├── LICENSE
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── GitHub Release (.exe)
└── CI release workflow

v0.1.1 完善
├── CI test workflow
├── Dependabot
├── 徽章
└── Screenshots / GIF

v0.2.0 功能
└── 按社区反馈选代重要功能
```

---

## 七、关键决策点（需项目 owner 确认）

| 问题 | 选项 | 建议 |
|---|---|---|
| GitHub 用户名/组织名 | ？ | 创建 org 或用个人账号 |
| 作者名（LICENSE / Cargo.toml） | ？ | 填写实际作者名 |
| 首发是否带 `.exe` | GitHub Release / 不带 | 带，降低用户门槛 |
| 文档语言 | 英文 / 中英双语 | 英文首发 |
| 是否申请 crates.io 发布 | 是 / 否 | 否，工具类非库 |
| 代码签名 | 商业签名 / 暂不签 | 暂不签（成本高），说明替代方案 |
| 讨论区是否开启 | 是 / 否 | 是，收集反馈 |

---

## 八、验收标准（Done / Not Done）

- [ ] README.md 存在，包含功能介绍、安装、使用说明
- [ ] LICENSE 文件在仓库根目录
- [ ] CHANGELOG.md 记录 v0.1.0 内容
- [ ] CONTRIBUTING.md 存在
- [ ] SECURITY.md 存在
- [ ] `cargo test` 在 CI 通过
- [ ] GitHub Release 包含 `.exe` + `.zip`
- [ ] 无 Copyleft 依赖
