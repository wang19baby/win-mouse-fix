# 融合矩阵(终稿)

> 竞品原始调查见 `docs/openlogi-features.md`(借鉴/参考源)。
> 本文是融合决策终稿 + DPI 跨屏稳定移动需求分析 + 开发计划落点。
> 最后更新:2026-08-27

---

## 一、架构结论(为什么融合面窄)

| 维度 | win-mouse-fix(咱) | OpenLogi(竞品) |
|---|---|---|
| 干预层 | OS 级输入拦截:`WH_MOUSE_LL` / `WH_KEYBOARD_LL` + `SendInput` | 设备级协议:Logitech **HID++ 2.0**(USB/BT/接收器) |
| 作用对象 | **任意鼠标**(通用) | 仅 Logitech 设备 |
| 数据来源 | 系统注入的鼠标事件 | 设备 Feature 表(`0x0000`…`0x8320`) |
| 愿景 | "让普通鼠标超越妙控板" | 配置 Logitech 设备本身 |

竞品约 95% 功能(灯光/RGB、Crown 旋钮、触觉、主机切换、触控板、耳机音频、太阳能、Onboard 配置)均为**设备级 HID++ 配置**,与咱"拦截 OS 事件让普通鼠标更好用"的路线正交——融了等于把咱做成 Logitech 驱动,背离愿景。

**真正可融的只有一条主线:设备可读数据(电量、硬件 DPI)的"读取 + 呈现 + 联动"**,不碰设备配置(除用户明确要的 DPI 档位设置)。这引出项目第二个架构支柱——**设备通信层(`device/`)**,与既有 OS 拦截层并存。

---

## 二、融合决策矩阵(终稿)

| 竞品 Feature | 融合判定 | 咱侧实现机制 | 落地 Phase | 备注 |
|---|---|---|---|---|
| 电量 `0x1004`/`0x1000`/`0x1001` | **融合** | `device/` HID++ 读取 → 托盘动态图标 | P8 + P9 | 需新增设备层;事件订阅免轮询 |
| DPI `0x2201`/`0x2202` | **融合(用户新增)** | `device/` HID++ 设硬件 DPI + 跨屏自动切换 | P8 + P10 | 用户需求:档位配置 + 跨屏稳定移动 |
| 滚轮反转/HiRes `0x2121` | **部分** | 反转=OS 级(config)已可做;HiRes 分辨率=设备级 | P1 / P8 | 反转无需设备层;HiRes 需 HID++ |
| 按键 remap 数据模型 `CidReporting` | **借鉴模型** | OS 级改写(`SendInput`),非设备 remap | P2 | 借用 `remap`/`diverted` 结构体,机制不同 |
| 指针 `0x2200` MousePointer | **部分** | OS 灵敏度替代(`SPI_SETMOUSESPEED`) | P5 | 设备级硬件指针咱不碰 |
| SmartShift `0x2110/0x2111` | 不融合(暂) | 滚轮模式为设备级 Logitech | — | 范围外,后续可选 |
| 灯光/RGB `0x8070`/`0x8071`/`0x8081` | 不融合 | Logitech 设备配置 | — | 偏离愿景 |
| Crown `0x4600` / 触觉 `0x19b0` / 主机切换 `0x1814` | 不融合 | Logitech 设备专有 | — | 偏离愿景 |
| 触控板 `0x6100`/`0x6110` / 音频 `0x8300+` / 太阳能 `0x4301` / Onboard `0x8100` | 不融合 | 设备专有 | — | 偏离愿景 |

---

## 三、借鉴的架构模式(非功能,直接搬)

一旦落地 `device/` 层,以下竞品模式值得直接复用,避免每 Feature 写样板:

1. **Feature Registry 宏**(`known_features!`):集中注册 Feature ID,编译期校验 ID↔实现类型匹配,typo 不可能注册到错误 ID。
2. **Probe 降级 + 缓存**:探测阶段读特征表,按需读能力,支持部分能力缺失的优雅降级;结果缓存复用,避免重复 HID++ 往返。电量优先级 `0x1004 → 0x1000 → 0x1001`。
3. **EventSource + DecodeEvent**:电量等支持主动上报的 Feature 用 channel listener 接收广播事件,免轮询、省电、实时。
4. **`CidReporting` / `CidReportingChange` 数据模型**:Phase 2 remap 可借用其 `remap`/`diverted`/`raw_xy` 字段设计(仅作数据模型,执行仍走 OS 级 `SendInput`)。
5. **`Capabilities` DTO**:从"设备支持哪些 Feature"派生能力标志(如 `battery`/`pointer`/`hires_wheel`),UI 按能力显隐,而非按设备型号。

---

## 四、DPI 跨屏稳定移动(用户新增需求分析)

### 4.1 问题
不同显示器**物理像素密度(PPI)**不同。鼠标硬件 DPI 固定、OS 灵敏度固定时,光标速度 = `硬件DPI × OS灵敏度`(像素/手部英寸)。进入更密的显示器后,同样像素数对应更小**物理**距离 → 光标"显得更快"。用户要的是**跨屏物理移动手感一致**。

### 4.2 公式
设第 $i$ 屏物理密度为 $P_i$(像素/英寸),目标硬件 DPI 为 $D_i$,OS 灵敏度 $S$ 固定。
物理光标速度 $\propto D_i \cdot S / P_i$,令其跨屏恒定 $\Rightarrow D_i \propto P_i$。

即:**进入某屏时,把鼠标硬件 DPI 设为与该屏物理密度成正比的值**(相对参考屏缩放,并夹在设备 `AdjustableDpi` 的 `[min,max]` 内)。

### 4.3 机制
- **读屏密度**:`GetDpiForMonitor(hmon, MDT_RAW_DPI, &dpiX, &dpiY)` 拿原生 DPI(基于分辨率 + EDID 物理尺寸,免 EDID 解析)。绝对物理尺寸回退用 WinRT `Windows.Graphics.Display.DisplayMonitor.PhysicalSize`。
- **触发切换**:光标移动时 `MonitorFromPoint(GetCursorPos)` 判屏变化;或监听 `WM_DISPLAYCHANGE`(增删屏)。
- **设硬件 DPI**:HID++ `0x2201` `AdjustableDpi`(`getSensorDpi`/`setSensorDpi`)或 `0x2202` `ExtendedAdjustableDpi`。**Logitech 设备级**,需 `device/` 层。
- **配置**:`[dpi]` 段——`auto_switch`(开关)、`base_dpi`(参考屏基准)、可选 `levels`(预设档位,供托盘/快捷键手动切)。

### 4.4 风险 / 边界
- 仅 Logitech(及暴露硬件 DPI 的设备)可做硬件档位;杂牌 2.4G 鼠标无此能力 → 该场景下退化为 OS 灵敏度补偿(`SPI_SETMOUSESPEED`,粗调)或禁用。
- 硬件 DPI 切换有延迟/跳变,需做**去抖**(屏变化后短延迟 + 阈值,避免拖拽中抖动)。
- 多鼠标场景:只对"当前被追踪的设备"生效。

---

## 五、开发计划落点

| 新增 Phase | 内容 | 依赖 |
|---|---|---|
| **Phase 8 — 设备通信层** | `src/device/`:Logitech HID++ 枚举/接收器探测、Feature Registry + Probe 降级、电量缓存、`AdjustableDpi` 读写、事件订阅 | 无(独立支柱) |
| **Phase 9 — 电量托盘图标** | 动态 `HICON`(电量条自绘)→ `NIM_MODIFY` + timer/事件刷新;`szTip` 显示百分比 | P8 |
| **Phase 10 — DPI 档位 + 跨屏自动切换** | `[dpi]` 配置;屏密度探测;`MonitorFromPoint` 判屏;`setSensorDpi` 联动 | P8 |

详细计划见 `PLAN.md`(Phase 8/9/10 已并入)。
