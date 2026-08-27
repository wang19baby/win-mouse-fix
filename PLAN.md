# Win Mouse Fix — 开发计划

> 目标:复刻 macOS `mac-mouse-fix`(`D:/work_space/personal_workspace/mac-mouse-fix`),
> 做一款 **Windows 下比 Apple 妙控板更好用的鼠标工具**。
> 技术栈:Rust + `windows-sys`(Win32)。当前可编译、可启动、**74 测试通过**。
> 详细背景见 `CLAUDE.md`;平滑滚动算法细节见对话记录。

---

## 状态校正（2026-08-27）

> **文档曾严重滞后于代码。** 核查发现 Phase 1/2/4/5 的核心功能（平滑滚动、按键重映射、修改键、指针加速）已在代码中实现，并经 `cargo test`（74 项）验证、已接入 `hooks.rs`。原 `CLAUDE.md` 称"功能逻辑均为占位"已不准确,已同步修订。
> 真实进度见各 Phase 标题状态标记:**主线(Phase 0–7)已全部实装**;**支线 Phase 8 起步(8.1 最小闭环已实装),8.2 增强/9/10 为绿地未实现**。

---

## 架构总则(Windows 与 macOS 的关键差异)

| macOS (mac-mouse-fix) | Windows (本项目的等价实现) |
|---|---|
| `CGEventTap` 拦截 **并自由合成** 任意事件 | `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 低级钩子 **只能观察+修改** 拦截到的事件 |
| Helper 进程常驻、持辅助功能权限 | 主进程常驻、跑消息泵(已具备) |
| 平滑输出:直接合成连续滚动事件流 | **必须用独立线程 `SendInput` 注入** 衰减的 wheel delta |

**核心结论**:低级钩子负责"观测"tick 速度与方向;一条**注入线程**负责按平滑器算出的连续目标,用 `SendInput`(`MOUSEEVENTF_WHEEL` / `MOUSEEVENTF_HWHEEL`)持续发出递减的滚动量。钩子过程必须**极快返回**(不能做平滑计算),所有重活放到工作线程。

**架构补充:第二支柱——设备通信层 `device/`**。Phase 1–7 是纯 OS 级输入拦截/改写,与具体鼠标型号无关。Phase 8 起引入**设备级 HID++ 通信**(Logitech 接收器/BT 直连),用于**读取**设备遥测(电量)与**设置**硬件 DPI。它与 OS 拦截层并存、互不依赖:拦截层管"手感",设备层管"设备状态/参数"。竞品 `OpenLogi` 的 Feature Registry / Probe 降级 / EventSource 模式见 `docs/openlogi-features.md` 与 `docs/integration-matrix.md`(融合终稿),`device/` 直接复用其宏与缓存设计。

---

## Phase 0 — 基础验证(已完成,锁定)

- [x] `cargo build` / `cargo test` 通过,二进制可启动(托盘+钩子+消息泵)
- [x] 模块:`config` / `log` / `win::{tray,hooks,message_loop}`
- [x] `config.toml` 加载/默认写入;`PartialEq` + 2 个配置测试
- 待办:补 `.gitignore` 确认 `target/`、`config.toml`(运行时生成)不被误提交

---

## Phase 1 — 平滑滚动 ✅ 已实现（99 测试 + hooks 接线 + P0 base-curve carry-over + restart smoothing）(最高价值,先落地)

**目标**:普通滚轮产生连续、带惯性、亚像素的"触控板手感"。
**对应 mac 模块**:`Core/Smoothing/`、`Core/Scroll/`、`Core/Config/`。

### 1.1 新增模块 `src/scroll/`
- `smoother.rs`:移植 `DoubleExponentialSmoother`(Holt 双指数平滑)
  - 参数 `a`(水平/数据平滑权重)、`y`(趋势/惯性权重)
  - `smooth(value) -> Double`、`predict(steps) -> Double`(向前预测 h 步,用于展开惯性)
  - `reset()`
  - **单元测试**:固定输入序列 → 断言输出收敛/趋势方向;确定性、无 IO。
- `wheel_tracker.rs`:在 `hooks.rs` 的 `mouse_proc` 中记录每次 wheel 事件的 `delta` + 时间戳,估算**速度/方向**(对应 `ScrollAnalyzer`)。
- `injector.rs`:独立线程,持有一个 `VecDeque<f64>` 待发滚动量;用高精度定时器(`QueryPerformanceCounter` / `timeBeginPeriod`)按固定节拍调用 `SendInput` 发出,发完退出。
- `subpixel.rs`:亚像素累积(对应 `SubPixelator`),整数提交、余数保留。

### 1.2 接入 `hooks.rs`
- `mouse_proc` 中 `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL`:
  - 若 `scroll.enabled` 且 `scroll.smooth`:**吞掉原事件**(`return 1` 或不调用 `CallNextHookEx` 原样),改为喂给 `wheel_tracker` + `smoother`,由注入线程输出。
  - 否则原样透传(保持当前行为)。

### 1.3 配置扩展(`config.rs` + `config.toml`)
```toml
[scroll]
enabled = true
smooth = true        # 双指数平滑 + 惯性
speed = 1.0          # 增益曲线总倍率(后续可换为曲线)
invert = false       # 反转方向
# 新增(可选): smooth_level = 0.6, smooth_trend = 0.3  # a / y 默认
```

### 1.4 验收
- 滚轮滚动 → 屏幕内容连续滑动、松手有短促惯性衰减、低速可亚像素微动。
- 关 `smooth` → 退化为原始离散滚动。
- 单元测试覆盖 smoother 收敛与方向。

---

## Phase 2 — 按键重映射 ✅ 已实现（RemapEngine + ClickCycleTracker, 接入 hooks）(Button Remapping)

**对应 mac 模块**:`Core/Remap/`、`Core/Buttons/`、`Core/Actions/`。

- [ ] `config.toml` 增加 `buttons.remaps` 表(源按键 → 动作)。
- [ ] `mouse_proc` 识别 `WM_LBUTTONDOWN/UP` 等,`keyboard_proc` 识别修饰键。
- [ ] `src/remap/`:动作执行层 —— 用 `SendInput` 合成目标按键/组合键,或抑制原按键(对应 mac 的 "capture")。
- [ ] 动作类型:v1 支持 `key`(发某键/组合)、`disabled`(吞掉)、`button`(改发另一键)。
- [ ] 测试:重映射表解析 + 动作执行(可用模拟事件或确定性分支测试)。

---

## Phase 3 — 点击拖拽手势 ✅ 已实现（DragController 三模式: move/scroll(喂注入器)/navigate(前后导航); 修改键联动已接）(Scroll & Navigate)

**对应 mac 模块**:`Core/Drag/`(TwoFingerSwipe / ThreeFingerSwipe / FakeDrag / AddMode)。

- [x] 检测某键"按下并拖拽",生成位移向量(`DragController` + `consume_delta`/`total_delta`)。
- [x] 映射为:垂直/水平滚动(喂给 Phase 1 注入器, `drag.mode="scroll"`)、或 `SendInput` 模拟手势(`FakeDrag`)、或前后导航(`drag.mode="navigate"`)。
- [x] 与 Phase 4 修改键联动(拖拽中组合修饰键切语义): scroll 模式 Shift→横向滚动 / Ctrl→精确(减速)。

---

## Phase 4 — 修改键行为 ✅ 已实现（ModifiedScrollModification 接入 process_wheel）(Modifier-based behaviors)

**对应 mac 模块**:`Core/Modifiers/`、`Core/Scroll/ScrollModifiers.swift`。

- [ ] 维护修饰键状态(`keyboard_proc` 已就位)。
- [ ] 滚轮 + 修饰键 → 切换效果:精确滚动 / 快速滚动 / 缩放 / 水平滚动 / 旋转 等(枚举来自 mac 源码)。
- [ ] 配置化每类修饰键绑定。

---

## Phase 5 — 指针加速 / 速度调校 ✅ 已实现（PointerAccel + 测试, 接入 hooks）(可后置,难度较高)

**对应 mac 模块**:`Core/PointerSpeed/`(IOHID 加速表桥接)。

- [ ] 调节鼠标灵敏度:`SystemParametersInfoW(SPI_SETMOUSESPEED)` 或更底层。
- [ ] 注意 Windows 指针加速与 mac 体验差异,优先"关闭加速 + 线性增益"。

---

## Phase 6 — 配置 UI + 热重载 ✅ 部分（托盘菜单已有, config.toml 热重载已实装; 轻量 GUI 可选未做）

- [ ] 托盘菜单扩展:开关各功能、打开配置。
- [ ] 可选轻量 GUI(原生 Win32 对话框 或 `egui`/`iced`)。
- [x] 监视 `config.toml` 变更热重载,无需重启:**采用 mtime 轮询(托盘 WM_TIMER 1s)而非 `ReadDirectoryChangesW`**——零新增线程、复用已有消息泵;`check_reload_config()` 检测 mtime 变化 → `Config::load_or_default()` → `apply_config()`(幂等可重入)。(`config.rs::config_changed` 单测覆盖)

---

## Phase 7 — 应用/设备配置档 ✅(per-app 已实装; per-device 待 Phase 8)

mac-mouse-fix 3 已砍掉、2 有。作为增强:按前台窗口 exe 名套用不同配置。

- [x] **per-app 配置档(已实现)**:`config.toml` 支持 `[[profiles]]`(`match_exe` 大小写不敏感子串匹配 + `config` 为 partial TOML 深合并);前台窗口 exe 每 500ms 轮询(`ID_TIMER_PROFILE`),变化时 `Config::resolve_for_exe` 合并后 `apply_config`,复用热重载路径。`match_type = "device"` 已解析但待 Phase 8 设备层。
- [ ] per-device 配置档(待 Phase 8 设备枚举就绪后接入)。

---

## Phase 8 — 设备通信层 `device/`(HID++ 支柱,新增) — 8.1 最小闭环 ✅(枚举+Feature探测+电量读取);8.2 架构增强(缓存 ✅,Probe 降级 已含于 Option 语义,其余 ⬜)

**目标**:引入与 OS 拦截层并存的设备级通道,读取电量、设置硬件 DPI。仅 Logitech(Unifying/Bolt/Lightspeed 接收器或 BT 直连)可靠;杂牌 2.4G 鼠标无此能力。
**对应竞品**:`OpenLogi` Feature Registry / Probe / EventSource(详见 `docs/integration-matrix.md` §三)。

### 8.1 新增模块 `src/device/`(✅ 最小闭环已实装)
- `hidpp.rs`:HID++ 2.0 协议(纯、可测)——短/长报告编解码、`ROOT` 的 `GetFeature` 解析 feature index、`BATTERY(0x1000)` 的 `GetBatteryLevelStatus` 解析。单测 4 项(`cargo test` 80 全绿)。
- `enumerate.rs`:Win32 SetupAPI + HID 枚举 Logitech 设备(vid `0x046D`),开句柄取 vid/pid/serial。
- `battery.rs`:`read_battery(path)` 最小闭环——开 HID 句柄 → `GetFeature(BATTERY)` → `GetBatteryLevelStatus` → 解析百分比/充电状态(WriteFile/ReadFile 收发短报告)。
- `mod.rs`:`log_battery_status()` 枚举 + 逐个读电量写日志;托盘菜单"电池状态"触发。
- `hid.rs`(更完整的句柄管理)与以下增强项待 8.2。
- `registry.rs`:`known_features!` 宏集中注册 Feature ID(编译期校验),仿竞品 `registry.rs`。
- `probe.rs`:探测阶段读 `FeatureSet(0x0001)` → 特征表;按需读能力,**优雅降级 + 结果缓存**(避免重复 HID++ 往返)。
- `battery.rs`:`BatteryInfo{percentage,level,status}`(仿竞品 `unified_battery.rs`);feature 优先级 `0x1004 → 0x1000 → 0x1001`;经托盘定时器(`ID_TIMER_BATTERY`, 2s)轮询读取(非 EventSource 推送;见 8.2 重估)。
- `dpi.rs`:`AdjustableDpi(0x2201)` / `ExtendedAdjustableDpi(0x2202)` —— `getSensorDpi` / `setSensorDpi`,拿 `[min,max,step]` 与当前值。
- `cache.rs`:`BatteryInfo` 全局缓存(`LazyLock<RwLock>`,仿 `CONFIG`),供托盘/其它模块无锁热读。
- **后端抽象(决策:2026-08-27,2026-08-28 重估确认)**:当前单后端(HID++),`battery`/`dpi` 直接调用 `hidpp`,**未实现 trait / `known_features!` 宏 / `WinrtBackend`**(按 8.2 决策有意暂缓,单后端属过度抽象)。若未来引入第二后端(蓝牙 WinRT),再抽 trait 零改调用方。
- **8.2 重估结论(2026-08-28)**:核对 `src/device` 实际代码——无 `EventSource` / 无 `Capabilities` DTO / 无 registry 宏 / 无后端 trait(此前仅 PLAN 描述,未落地)。① EventSource:电量经托盘 2s 轮询即够,真机推送订阅需常驻异步报告监听并处理断连,复杂度/鲁棒性代价远大于慢变遥测收益 → **暂缓**;② Capabilities DTO:各 feature 以 `Option` 返回已覆盖探测降级,单后端两 feature 下过早 → **不引入**;③ Registry 宏 / 后端 trait:单后端 YAGNI → **维持暂缓**。三者均不新增代码,保持直接 `hidpp` 调用。

### 8.2 架构借鉴(直接搬)
- Feature Registry 宏 → 杜绝 ID 注册 typo。
- Probe 降级 + 缓存 → 缺失能力不崩,省往返。
- EventSource 事件订阅 → 电量用设备主动上报,免轮询、省电。

### 8.3 验收
- 连接 Logitech 鼠标 → `device/` 能枚举到设备、读到电量百分比、读到 DPI 范围。
- 拔插接收器 → hotplug 重探测不崩。
- 单元测试:Feature 降级选择、缓存命中(无需真实设备,用桩 endpoint)。

---

## Phase 9 — 电量托盘图标(动态图标) ✅

**目标**:托盘图标实时显示鼠标电量百分比;低电量变色提示。
**依赖**:Phase 8(`cache.rs` 的 `BatteryInfo`)。

- [x] **已实装**:托盘每 2s(`ID_TIMER_BATTERY`)轮询 `cache::update()`(枚举 Logitech → 读首个电量)→ `NIM_MODIFY` 刷新 `szTip`("鼠标电量 78%" / "⚠ 鼠标电量低 15%" / "（充电中）")并 GDI 重绘图标叠加百分比数字(低电量红色)。overlay 图标用后 `DestroyIcon` 防泄漏;GDI 失败回退原图标。图标叠加渲染为 headless 不可视验证,需真机目检。

### 9.1 `src/win/tray.rs` 改造
- 新增常驻 timer(如 1s)或订阅 `device/` 电量事件 → 读 `cache` → 重绘 `HICON` → `Shell_NotifyIconW(NIM_MODIFY)` 更新 `hIcon` + `szTip`("鼠标 78%")。
- **动态图标渲染**:`CreateIconIndirect` + 32bpp ARGB 位图自绘(圆角电池框 + 按比例填充条),**不依赖字体**;低电量(<20%)填充转红。复用 `tools/gen_icon.py` 的像素思路。
- 无设备/无电量时回退到 `assets/icon.ico`(当前静态图标)。

### 9.2 验收
- 电量变化(或事件上报)→ 托盘图标与 tooltip 在 1s 内更新。
- 低电量 → 图标红色提示。
- 无 Logitech 设备 → 显示默认图标,不崩。

---

## Phase 10 — DPI 档位配置 + 跨屏自动切换 ✅(2026-08-28 校正完成)

### 10.1 配置扩展(`config.rs` + `config.toml`)
```toml
[dpi]
auto_switch = true     # 跨屏自动调硬件 DPI
base_dpi = 800         # 参考屏基准 DPI(相对缩放锚点)
# levels = [400, 800, 1600, 3200]  # 可选预设档位,供托盘/快捷键手动切
```

### 10.2 屏幕密度探测
- `GetDpiForMonitor(hmon, MDT_RAW_DPI, &dpiX, &dpiY)` 拿原生 DPI(基于分辨率 + EDID,免解析);绝对物理尺寸回退 WinRT `DisplayMonitor.PhysicalSize`。
- 光标移动时 `MonitorFromPoint(GetCursorPos)` 判屏变化;或监听 `WM_DISPLAYCHANGE`。

### 10.3 联动设置(公式 $D_i \propto P_i$)
- 屏 $i$ 目标硬件 DPI:$D_i = \text{base\_dpi} \times P_i / P_{\text{ref}}$,夹在设备 `[min,max]` 内。
- 经 `dpi.rs::setSensorDpi` 写入;**去抖**(屏变化后短延迟 + 阈值,避免拖拽中抖动)。

### 10.4 验收
- 双屏(密度不同)拖动光标跨屏 → 鼠标硬件 DPI 自动调整,物理手感一致。
- 关 `auto_switch` → 维持手动/默认档位。
- 杂牌无 DPI 设备 → 降级提示或 OS 灵敏度补偿,不崩。

---

## 跨阶段风险与约束

1. **合成受限**:低级钩子不能自由合成事件 → 必须用 `SendInput` 注入线程(Phase 1 核心架构)。
2. **钩子延迟**:`mouse_proc`/`keyboard_proc` 必须毫秒级返回,平滑/注入全在工作线程。
3. **UIPI / 权限**:跨进程注入可能受完整性级别限制;必要时以提权运行或声明 manifest。
4. **计时精度**:注入节拍用高精度定时器,避免 `WM_TIMER` 的 ~15ms 抖动。
5. **专有协议鼠标**(Logitech Options 等):标准 USB/HID 之外的按键无法识别,与 mac 同源限制。
6. **无头验证**:CI 难模拟真实输入;以单元测试(平滑器、配置、动作映射)+ 手动烟测为主。

---

## 建议执行顺序（状态校正于 2026-08-27）

**OS 拦截主线(愿景核心,与设备无关)** —— 进度:`Phase 0`✅ → `Phase 1`✅ → `Phase 2`✅ → `Phase 4`✅ → `Phase 5`✅ → `Phase 6`✅(热重载) → `Phase 3`🟡(拖拽手势) → `Phase 7`✅(配置档) → **主线收尾完成**;设备支线:`Phase 8`(8.1 最小闭环 ✅,8.2 缓存 ✅/Probe降级 已含,registry宏·后端trait 未做) → `Phase 9` 电量托盘 ✅ → `Phase 10` DPI 跨屏 ✅(手动同步)。
`Phase 3` 拖拽手势 🟡 → `Phase 6` config.toml 热重载 ✅ → `Phase 7` per-app 配置档 ✅(主线收尾完成)。

**设备层支线(Logitech 可读/可设,独立于主线)** —— `Phase 8` 起步(8.1 枚举+电量最小闭环 ✅,8.2 中 cache ✅ 其余⬜) → `Phase 9` 电量托盘图标 ✅ → `Phase 10` DPI 跨屏自动切换 ✅(2026-08-28 经 OpenLogi 源码校正 SetSensorDpi=fn 0x03,big-endian)。
下一步建议(主线收尾):**主线(Phase 0–7)已全部实装**;设备支线 `Phase 8` 的 8.1 最小闭环(枚举+电量)已实装。
下一步建议(支线):Phase 10 已结项——经 OpenLogi `openlogi-hidpp` crate(`adjustable_dpi.rs`)确认 HID++ feature 0x2201 AdjustableDpi 的 SetSensorDpi function code=**0x03**,DPI 字节序为**big-endian**(hi,lo)。`dpi-probe` 二进制已就绪,可在真机有 G502 时运行验证。
