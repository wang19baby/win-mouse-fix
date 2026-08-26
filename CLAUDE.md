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

脚手架已搭好，功能逻辑均为占位：

- `src/main.rs` —— 入口：加载配置 → 建托盘 → 装钩子 → 跑消息循环 → 卸载钩子
- `src/config.rs` + `config.toml` —— TOML 配置（general / scroll / buttons，目前全禁用）
- `src/log.rs` —— 文件 + stderr 日志
- `src/win/tray.rs` —— 系统托盘（消息-only 窗口，右键菜单：关于 / 退出）
- `src/win/hooks.rs` —— `WH_MOUSE_LL` + `WH_KEYBOARD_LL` 低级钩子（事件目前原样透传）
- `src/win/message_loop.rs` —— 标准 Win32 消息泵

### Windows 对应关系

| mac（事件拦截） | win（等价机制） |
|---|---|
| `CGEventTap`（Helper） | `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 低级钩子（已在 `hooks.rs`） |
| 辅助功能权限 | 需以管理员/辅助功能方式运行；UIPI 注意 |
| 主 App UI | 暂定：托盘 + 未来配置 UI |
| 文件配置 + FileMonitor | `config.toml` + 热重载 |
| `GlobalEventTapThread` | `message_loop.rs` 消息泵线程 |

## 后续工作方向（占位 → 实现）

按 mac 源码模块逐步落地到 Rust：
1. **平滑滚动**：`Smoothing/` → 在 `mouse_proc` 中改写 wheel delta，加指数/双指数平滑器。
2. **按键重映射**：`Remap/` + `Buttons/` → 解析 config 的重映射表，改写按键事件。
3. **点击拖拽手势**：`Drag/` → 将按键的"点击并拖拽"转成滚动/导航。
4. **指针加速**：`PointerSpeed/` → 调整鼠标灵敏度（难度较高，可后置）。
5. **修改键组合**：`Modifiers/` → 按住修饰键切换滚动/按键行为。
6. **配置 UI**：托盘菜单扩展 + 可选 GUI。

> 注：Win32 低级钩子无法拦截所有 mac 能拦截的事件（如某些专有协议鼠标），
> 且改写能力有限（不能像 `CGEventTap` 那样自由合成触控板手势），
> 部分手势需改用 `SendInput` 合成。具体能力边界在实现时再确认。
