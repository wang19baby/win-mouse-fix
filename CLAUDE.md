# Win Mouse Fix

## 愿景 / Vision

复刻 macOS 上的 **Mac Mouse Fix**（`D:/work_space/personal_workspace/mac-mouse-fix`），
目标是做一款 **Windows 下的鼠标工具**，让它比 Apple 妙控板（Magic Trackpad）更好用。

> Mac Mouse Fix 的口号即 "Make Your $10 Mouse Better Than an Apple Trackpad!" ——
> 本项目在 Windows 上追求同一目标：让普通鼠标获得超越妙控板的交互体验
> （平滑滚动、手势化按键、指针加速调校、滚轮/修改键组合等）。

## 目标源码（参考实现）

`mac-mouse-fix` 是大型 Obj-C / Swift (Xcode) 项目，核心逻辑在 `Helper/Core/`，
由一个拥有辅助功能（Accessibility）权限的 **Helper 进程** 通过 `CGEventTap` 拦截全局输入并改写：

| 模块（mac 路径） | 职责 |
|---|---|
| `Helper/Core/Scroll/` | 滚动轮事件处理、滚动修改键、滚动分析 |
| `Helper/Core/Smoothing/` | 平滑滚动：指数/双指数平滑、滚动平均、环形缓冲 |
| `Helper/Core/Remap/` | 按键重映射（Remap、RemapSwizzler） |
| `Helper/Core/Buttons/` | 按键输入接收、按键修改键、点击周期、Remaps 分析 |
| `Helper/Core/Drag/` | 点击拖拽手势 → 双指/三指滑动、FakeDrag、AddMode 输出 |
| `Helper/Core/PointerSpeed/` | 指针加速（IOHID 加速表桥接） |
| `Helper/Core/Modifiers/` | 修改键状态 |
| `Helper/Core/Coordinate/` | SwitchMaster / OutputCoordinator 协调多类输出 |
| `Helper/Core/Actions/` | 实际动作（前进/后退、SymbolicHotKeys 等） |
| `Helper/Core/Touch/` | 触摸板手势模拟 |
| `Helper/GlobalEventTapThread` + `EventUtility` + `ModificationUtility` | 事件拦截与改写主循环、IPC |
| `App/`（主程序） | UI：Scroll / Pointer / Button(RemapTable) / General / About 标签页 + AppState |

**架构要点**：Helper 负责拦截与改写（需辅助功能权限），主 App 仅做 UI 与配置；
配置通过文件 + MessagePort IPC + FileMonitor 共享。

## 当前实现（win-mouse-fix，Rust + Win32）

项目已超出脚手架阶段。运行时代码、关键缺口和测试结果以 `ROADMAP.md` 及
`docs/architecture-stability-optimization-evidence.md` 的最近一次实查为准：

- `src/main.rs` —— 入口：加载配置 → 建托盘 → 装钩子 → 可选启动 LAN 服务 → 消息循环 → 有序停止
- `src/config.rs` + `config.toml` —— TOML 配置、每应用 profile、原子保存与热重载
- `src/log.rs` —— 文件 + stderr 日志
- `src/win/tray.rs` —— 系统托盘：全局功能开关、录制映射、手机妙控板、帮助、关于、退出
- `src/win/hooks.rs` —— `WH_MOUSE_LL` + `WH_KEYBOARD_LL`；wheel 接平滑管线，button 接重映射与拖拽
- `src/win/message_loop.rs` —— Win32 消息泵及跨线程 WinEvent hook 请求
- `src/scroll/` —— 平滑滚动、惯性曲线、亚像素和专用 `SendInput` 注入线程
- `src/remap/` —— `RemapEntry` / `ClickCycleTracker` / `RemapEngine` / effect 执行
- `src/accel/` —— 指针加速数学与状态控制器；当前尚未接入 `WM_MOUSEMOVE`
- `src/gesture/` —— `DragController` 的 move/scroll/navigate 输出，已接入鼠标钩子
- `src/remote.rs` + `assets/` —— 可选 LAN HTTP/WebSocket 手机触控板；32 连接上限、有界广播队列、CSPRNG bearer，传输仍为明文且仅适合可信局域网
- `src/device/` —— Logitech 电量/DPI 查询；托盘刷新与跨屏写入定时器仍待真机验收后启用

> 发布配置默认启用核心滚动/按键路径；窗口拖拽、指针加速和 LAN 服务采用安全的显式启用。真实输入手感与 HID++ 写入仍必须在目标硬件上验收。

### Windows 对应关系

| mac（事件拦截） | win（等价机制） |
|---|---|
| `CGEventTap`（Helper） | `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 低级钩子（已在 `hooks.rs`） |
| 权限边界 | 普通桌面程序可在当前用户会话运行；若要作用于提权程序，需同级提权并注意 UIPI |
| 主 App UI | 暂定：托盘 + 未来配置 UI |
| 文件配置 + FileMonitor | `config.toml` + 热重载 |
| `GlobalEventTapThread` | `message_loop.rs` 消息泵线程 |

## 功能落地状态（运行时代码实查于 2026-09-11）

按 mac 源码模块逐步落地到 Rust；“代码存在”与“运行时已接入/真机已验收”分开标记：

| # | 功能 | 对应 mac 模块 | 实查状态 | 对应 Phase |
|---|---|---|---|---|
| 1 | 平滑滚动 | `Smoothing/` | [完成] 已接入钩子与专用注入线程 | P1 |
| 2 | 按键重映射 | `Remap/`+`Buttons/` | [完成] RemapEngine/ClickCycle/AddMode 已接入 | P2 |
| 3 | 点击拖拽手势 | `Drag/` | [完成] move/scroll/navigate 已接入；待真实鼠标手感验收 | P3 |
| 4 | 指针加速 | `PointerSpeed/` | [部分] 数学与状态控制器有测试，尚未接入移动事件 | P5 |
| 5 | 修改键组合 | `Modifiers/` | [完成] ModifiedScrollModification 已接入 | P4 |
| 6 | 配置 UI + 热重载 | 托盘菜单 + GUI | [部分] 托盘和热重载可用；独立 GUI 未暴露 | P6 |
| 7 | 设备层（电量/DPI） | （竞品融合） | [部分] 查询/切换代码存在；显示与自动写入定时器未启用，需真机验收 | P8–P10 |
| 8 | 手机触控板 | `Touch/` 等价输出 | [部分] LAN HTTP/WS、认证和注入已接入；离线 PWA 与多设备真机验收未完成 | P13 |

> Phase 编号与本表顺序不完全一致。设备能力、指针加速和手机端兼容性不得仅凭单元测试宣称完成；以真实硬件/浏览器验收为最终门槛。

> 注：Win32 低级钩子无法拦截所有 mac 能拦截的事件（如某些专有协议鼠标），
> 且改写能力有限（不能像 `CGEventTap` 那样自由合成触控板手势），
> 部分手势需改用 `SendInput` 合成。具体能力边界在实现时再确认。
