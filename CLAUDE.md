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

脚手架已搭好，**核心功能已在代码中实现并经单元测试验证**（`cargo test` 99 项通过），非占位：

- `src/main.rs` —— 入口：加载配置 → 建托盘 → 装钩子 → 跑消息循环 → 卸载钩子
- `src/config.rs` + `config.toml` —— TOML 配置（general / scroll / buttons / drag / accel），含平滑/重映射/加速全套参数
- `src/log.rs` —— 文件 + stderr 日志
- `src/win/tray.rs` —— 系统托盘（消息-only 窗口，右键菜单：关于 / 退出 / 平滑滚动 / 按键重映射 / 录制映射）
- `src/win/hooks.rs` —— `WH_MOUSE_LL` + `WH_KEYBOARD_LL` 低级钩子；wheel 接入平滑注入管线，button 接入重映射引擎，move 接入指针加速
- `src/win/message_loop.rs` —— 标准 Win32 消息泵
  - `src/scroll/` —— 平滑滚动引擎：`smoother`(双指数) / `wheel_tracker` / `injector`(SendInput 线程) / `engine` / `curve` / `subpixel`，99 测试通过
- `src/remap/` —— 按键重映射：`RemapEntry` / `RemapTable` / `ClickCycleTracker` / `RemapEngine` / `execute_effect`
- `src/accel/` —— 指针加速：`accel_factor` + `PointerAccel`（纯函数 + 测试），经 `injector::send_mouse_move` 接入
- `src/gesture/` —— 窗口拖拽手势：`DragController`（部分接入）
- `src/modifiers.rs` —— 修改键状态（`ActiveModifiers` 等），已接入 `process_wheel`

> 注：已实现功能**默认配置多为禁用**（`config.toml` 中 `scroll.enabled` 等），需开启后做真实输入烟测确认验收。设备层（电量读取/托盘显示、DPI 读取）已实现，见 PLAN.md Phase 8–10；自动跨屏切换待实现。

### Windows 对应关系

| mac（事件拦截） | win（等价机制） |
|---|---|
| `CGEventTap`（Helper） | `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 低级钩子（已在 `hooks.rs`） |
| 辅助功能权限 | 需以管理员/辅助功能方式运行；UIPI 注意 |
| 主 App UI | 暂定：托盘 + 未来配置 UI |
| 文件配置 + FileMonitor | `config.toml` + 热重载 |
| `GlobalEventTapThread` | `message_loop.rs` 消息泵线程 |

## 功能落地状态（更新于 2026-08-27）

按 mac 源码模块逐步落地到 Rust，当前进度（详见 PLAN.md 各 Phase 标记）：

| # | 功能 | 对应 mac 模块 | 状态 | 对应 Phase |
|---|---|---|---|---|
| 1 | 平滑滚动 | `Smoothing/` | ✅ 已实现（smoother/injector/engine/curve/subpixel + 99 测试） | P1 |
| 2 | 按键重映射 | `Remap/`+`Buttons/` | ✅ 已实现（RemapEngine + ClickCycleTracker，接入 hooks） | P2 |
| 3 | 点击拖拽手势 | `Drag/` | ✅ 已实现（DragController: move/scroll(喂注入器)/navigate(前后导航) 三模式；修改键联动: scroll 模式 Shift→横向 / Ctrl→精确） | P3 |
| 4 | 指针加速 | `PointerSpeed/` | ✅ 已实现（PointerAccel + 测试，接入 hooks） | P5 |
| 5 | 修改键组合 | `Modifiers/` | ✅ 已实现（ModifiedScrollModification 接入 process_wheel） | P4 |
| 6 | 配置 UI + 热重载 | 托盘菜单 + GUI | ✅ 部分（托盘菜单已有；config.toml 热重载已实装；轻量 GUI 可选未做） | P6 |
| 7 | 设备层（电量/DPI） | （竞品融合，无 mac 对应） | 🟡 部分（电量读取+托盘显示、DPI 读取已实现；自动跨屏切换已实现(去抖轮询)；HID++ SetSensorDpi function 码(0x01 占位)待真机校正） | P8–P10 |

> 说明：Phase 编号与本文 1–6 顺序略有重排（PLAN 中 P4=修改键、P5=指针加速），语义一一对应。
> 设备层（电量托盘 / DPI 跨屏，Phase 8–10）为新增需求，源自竞品融合：电量读取与托盘显示、DPI 读取已实现；自动跨屏切换与 DPI 写入（真实 function 码）待实现。

> 注：Win32 低级钩子无法拦截所有 mac 能拦截的事件（如某些专有协议鼠标），
> 且改写能力有限（不能像 `CGEventTap` 那样自由合成触控板手势），
> 部分手势需改用 `SendInput` 合成。具体能力边界在实现时再确认。
