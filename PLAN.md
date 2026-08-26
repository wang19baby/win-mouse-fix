# Win Mouse Fix — 开发计划

> 目标:复刻 macOS `mac-mouse-fix`(`D:/work_space/personal_workspace/mac-mouse-fix`),
> 做一款 **Windows 下比 Apple 妙控板更好用的鼠标工具**。
> 技术栈:Rust + `windows-sys`(Win32)。当前脚手架可编译、可启动、2 测试通过。
> 详细背景见 `CLAUDE.md`;平滑滚动算法细节见对话记录。

---

## 架构总则(Windows 与 macOS 的关键差异)

| macOS (mac-mouse-fix) | Windows (本项目的等价实现) |
|---|---|
| `CGEventTap` 拦截 **并自由合成** 任意事件 | `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 低级钩子 **只能观察+修改** 拦截到的事件 |
| Helper 进程常驻、持辅助功能权限 | 主进程常驻、跑消息泵(已具备) |
| 平滑输出:直接合成连续滚动事件流 | **必须用独立线程 `SendInput` 注入** 衰减的 wheel delta |

**核心结论**:低级钩子负责"观测"tick 速度与方向;一条**注入线程**负责按平滑器算出的连续目标,用 `SendInput`(`MOUSEEVENTF_WHEEL` / `MOUSEEVENTF_HWHEEL`)持续发出递减的滚动量。钩子过程必须**极快返回**(不能做平滑计算),所有重活放到工作线程。

---

## Phase 0 — 基础验证(已完成,锁定)

- [x] `cargo build` / `cargo test` 通过,二进制可启动(托盘+钩子+消息泵)
- [x] 模块:`config` / `log` / `win::{tray,hooks,message_loop}`
- [x] `config.toml` 加载/默认写入;`PartialEq` + 2 个配置测试
- 待办:补 `.gitignore` 确认 `target/`、`config.toml`(运行时生成)不被误提交

---

## Phase 1 — 平滑滚动(最高价值,先落地)

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

## Phase 2 — 按键重映射(Button Remapping)

**对应 mac 模块**:`Core/Remap/`、`Core/Buttons/`、`Core/Actions/`。

- [ ] `config.toml` 增加 `buttons.remaps` 表(源按键 → 动作)。
- [ ] `mouse_proc` 识别 `WM_LBUTTONDOWN/UP` 等,`keyboard_proc` 识别修饰键。
- [ ] `src/remap/`:动作执行层 —— 用 `SendInput` 合成目标按键/组合键,或抑制原按键(对应 mac 的 "capture")。
- [ ] 动作类型:v1 支持 `key`(发某键/组合)、`disabled`(吞掉)、`button`(改发另一键)。
- [ ] 测试:重映射表解析 + 动作执行(可用模拟事件或确定性分支测试)。

---

## Phase 3 — 点击拖拽手势(Scroll & Navigate)

**对应 mac 模块**:`Core/Drag/`(TwoFingerSwipe / ThreeFingerSwipe / FakeDrag / AddMode)。

- [ ] 检测某键"按下并拖拽",生成位移向量。
- [ ] 映射为:垂直/水平滚动(喂给 Phase 1 的注入器)、或 `SendInput` 模拟手势。
- [ ] 与 Phase 4 修改键联动(拖拽中组合修饰键切语义)。

---

## Phase 4 — 修改键行为(Modifier-based behaviors)

**对应 mac 模块**:`Core/Modifiers/`、`Core/Scroll/ScrollModifiers.swift`。

- [ ] 维护修饰键状态(`keyboard_proc` 已就位)。
- [ ] 滚轮 + 修饰键 → 切换效果:精确滚动 / 快速滚动 / 缩放 / 水平滚动 / 旋转 等(枚举来自 mac 源码)。
- [ ] 配置化每类修饰键绑定。

---

## Phase 5 — 指针加速 / 速度调校(可后置,难度较高)

**对应 mac 模块**:`Core/PointerSpeed/`(IOHID 加速表桥接)。

- [ ] 调节鼠标灵敏度:`SystemParametersInfoW(SPI_SETMOUSESPEED)` 或更底层。
- [ ] 注意 Windows 指针加速与 mac 体验差异,优先"关闭加速 + 线性增益"。

---

## Phase 6 — 配置 UI + 热重载

- [ ] 托盘菜单扩展:开关各功能、打开配置。
- [ ] 可选轻量 GUI(原生 Win32 对话框 或 `egui`/`iced`)。
- [ ] 监视 `config.toml` 变更(`ReadDirectoryChangesW` / 轮询 mtime)热重载,无需重启。

---

## Phase 7 — 应用/设备配置档(可选)

mac-mouse-fix 3 已砍掉、2 有。作为增强:按前台窗口 exe 名套用不同配置。

---

## 跨阶段风险与约束

1. **合成受限**:低级钩子不能自由合成事件 → 必须用 `SendInput` 注入线程(Phase 1 核心架构)。
2. **钩子延迟**:`mouse_proc`/`keyboard_proc` 必须毫秒级返回,平滑/注入全在工作线程。
3. **UIPI / 权限**:跨进程注入可能受完整性级别限制;必要时以提权运行或声明 manifest。
4. **计时精度**:注入节拍用高精度定时器,避免 `WM_TIMER` 的 ~15ms 抖动。
5. **专有协议鼠标**(Logitech Options 等):标准 USB/HID 之外的按键无法识别,与 mac 同源限制。
6. **无头验证**:CI 难模拟真实输入;以单元测试(平滑器、配置、动作映射)+ 手动烟测为主。

---

## 建议执行顺序

`Phase 0`(已完成) → **`Phase 1` 平滑滚动**(最能体现"比妙控板好用")→ `Phase 2` 按键重映射 → `Phase 4` 修改键 → `Phase 3` 拖拽手势 → `Phase 6` UI → `Phase 5` 指针 → `Phase 7`。

下一步建议:从 **Phase 1.1 的 `smoother.rs`** 开始(纯算法、可单测、零平台依赖),再接 `injector.rs` 与钩子接线。
